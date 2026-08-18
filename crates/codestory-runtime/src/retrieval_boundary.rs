use std::path::Path;

pub use codestory_retrieval::{
    CacheCleanPlan, CacheCleanReport, FinalizeIndexOutcome, GenerationRetentionApplyReport,
    GenerationRetentionPlan, ProcessOwnerState, ProcessStartProbe, QueryResult,
    RetainedRollbackObservation, RetrievalIndexManifest, RetrievalStatusReport,
    RollbackActivationError, RollbackActivationOutcome, RollbackActivationRefusal,
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, SidecarGcReport, SidecarInventoryReport,
    SidecarProcessDefaults as RetrievalProcessDefaults,
    SidecarRuntimeDefaults as RetrievalRuntimeDefaults,
    SidecarRuntimeOverrides as RetrievalRuntimeOverrides,
};

/// Retrieval profile selected by an adapter when it constructs a runtime.
///
/// The retrieval crate owns how profiles map to artifact namespaces. Runtime
/// owns the public construction boundary so adapters do not import that
/// implementation type directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRetrievalProfile {
    Local,
    Agent,
}

impl RuntimeRetrievalProfile {
    /// Stable adapter-facing profile label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Agent => "agent",
        }
    }
}

impl From<RuntimeRetrievalProfile> for codestory_retrieval::SidecarProfile {
    fn from(profile: RuntimeRetrievalProfile) -> Self {
        match profile {
            RuntimeRetrievalProfile::Local => Self::Local,
            RuntimeRetrievalProfile::Agent => Self::Agent,
        }
    }
}

impl From<codestory_retrieval::SidecarProfile> for RuntimeRetrievalProfile {
    fn from(profile: codestory_retrieval::SidecarProfile) -> Self {
        match profile {
            codestory_retrieval::SidecarProfile::Local => Self::Local,
            codestory_retrieval::SidecarProfile::Agent => Self::Agent,
        }
    }
}

/// Immutable retrieval configuration supplied to one runtime.
///
/// Construction is intentionally mirrored here rather than re-exported. That
/// gives CLI consumers a runtime-owned type while retrieval retains artifact,
/// namespace, and engine-policy ownership.
#[derive(Debug, Clone)]
pub struct RuntimeRetrievalConfig(codestory_retrieval::SidecarRuntimeConfig);

impl RuntimeRetrievalConfig {
    pub fn local() -> Self {
        codestory_retrieval::SidecarRuntimeConfig::local().into()
    }

    pub fn for_project_auto(project_root: &Path) -> Self {
        codestory_retrieval::SidecarRuntimeConfig::for_project_auto(project_root).into()
    }

    pub fn for_project_auto_with_process_defaults(
        project_root: &Path,
        process_defaults: &RetrievalProcessDefaults,
        overrides: &RetrievalRuntimeOverrides,
    ) -> Self {
        codestory_retrieval::SidecarRuntimeConfig::for_project_auto_with_process_defaults(
            project_root,
            process_defaults,
            overrides,
        )
        .into()
    }

    pub fn for_project_auto_with_process_defaults_and_identity(
        project_identity: &codestory_workspace::ProjectIdentityV3,
        process_defaults: &RetrievalProcessDefaults,
        overrides: &RetrievalRuntimeOverrides,
    ) -> Self {
        codestory_retrieval::SidecarRuntimeConfig::for_project_auto_with_process_defaults_and_identity(
            project_identity,
            process_defaults,
            overrides,
        )
        .into()
    }

    pub fn for_project_profile(
        project_root: Option<&Path>,
        profile: RuntimeRetrievalProfile,
    ) -> Self {
        codestory_retrieval::SidecarRuntimeConfig::for_project_profile(project_root, profile.into())
            .into()
    }

    pub fn for_project_profile_with_run_id(
        project_root: Option<&Path>,
        profile: RuntimeRetrievalProfile,
        run_id: Option<&str>,
    ) -> Self {
        codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_run_id(
            project_root,
            profile.into(),
            run_id,
        )
        .into()
    }

    pub fn for_project_profile_with_process_defaults(
        project_root: Option<&Path>,
        profile: RuntimeRetrievalProfile,
        run_id: Option<&str>,
        process_defaults: &RetrievalProcessDefaults,
        overrides: &RetrievalRuntimeOverrides,
    ) -> Self {
        codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_process_defaults(
            project_root,
            profile.into(),
            run_id,
            process_defaults,
            overrides,
        )
        .into()
    }

    pub fn with_profile_and_run_id(
        &self,
        project_root: Option<&Path>,
        profile: RuntimeRetrievalProfile,
        run_id: Option<&str>,
    ) -> Self {
        self.0
            .with_profile_and_run_id(project_root, profile.into(), run_id)
            .into()
    }

    pub fn profile(&self) -> RuntimeRetrievalProfile {
        self.0.profile.into()
    }

    /// Retrieval-owned publication state observed by adapter status caches.
    ///
    /// The path is read-only at this boundary. Status adapters may fingerprint
    /// it, but cannot construct layouts or mutate retrieval state through the
    /// opaque runtime configuration.
    pub fn status_cache_state_path(&self) -> &Path {
        &self.0.layout.state_file
    }

    pub(crate) fn as_inner(&self) -> &codestory_retrieval::SidecarRuntimeConfig {
        &self.0
    }

    pub(crate) fn into_inner(self) -> codestory_retrieval::SidecarRuntimeConfig {
        self.0
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn as_raw_config_for_test(&self) -> &codestory_retrieval::SidecarRuntimeConfig {
        &self.0
    }
}

impl From<codestory_retrieval::SidecarRuntimeConfig> for RuntimeRetrievalConfig {
    fn from(config: codestory_retrieval::SidecarRuntimeConfig) -> Self {
        Self(config)
    }
}

/// Plan process-wide cache cleanup without mutating the cache tree.
pub fn plan_cache_clean() -> anyhow::Result<CacheCleanPlan> {
    codestory_retrieval::plan_cache_clean()
}

/// Apply process-wide cache cleanup under the retrieval owner's global lock.
pub fn apply_cache_clean() -> anyhow::Result<CacheCleanReport> {
    codestory_retrieval::apply_cache_clean()
}

/// Capture retrieval-owned process defaults for an adapter process.
pub fn retrieval_process_defaults() -> RetrievalProcessDefaults {
    codestory_retrieval::sidecar_process_defaults()
}

/// Initialize and validate the embedding backend selected by this runtime.
pub fn ensure_product_embedding_backend_for_runtime(
    config: &RuntimeRetrievalConfig,
) -> anyhow::Result<()> {
    codestory_retrieval::ensure_product_embedding_backend_for_runtime(config.as_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeProcessConfig;
    use codestory_contracts::workspace::SourceIndexPolicy;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn assert_structurally_equal(
        wrapped: &codestory_retrieval::SidecarRuntimeConfig,
        direct: &codestory_retrieval::SidecarRuntimeConfig,
    ) {
        assert_eq!(wrapped.project_identity, direct.project_identity);
        assert_eq!(wrapped.cache_root, direct.cache_root);
        assert_eq!(
            wrapped.layout.lexical_data_dir,
            direct.layout.lexical_data_dir
        );
        assert_eq!(
            wrapped.layout.semantic_data_dir,
            direct.layout.semantic_data_dir
        );
        assert_eq!(
            wrapped.layout.scip_artifacts_root,
            direct.layout.scip_artifacts_root
        );
        assert_eq!(wrapped.layout.state_file, direct.layout.state_file);
        assert_eq!(wrapped.profile, direct.profile);
        assert_eq!(wrapped.run_id, direct.run_id);
        assert_eq!(wrapped.namespace, direct.namespace);
        assert_eq!(wrapped.embedding, direct.embedding);
        assert_eq!(wrapped.retrieval, direct.retrieval);
        assert_eq!(wrapped.summary, direct.summary);
    }

    fn assert_constructor_pair(
        label: &str,
        wrapped: RuntimeRetrievalConfig,
        direct: codestory_retrieval::SidecarRuntimeConfig,
        configuration_root: &Path,
    ) {
        let policy = SourceIndexPolicy::default();
        let wrapped_id =
            RuntimeProcessConfig::new_with_retrieval_config(wrapped.clone(), policy.clone())
                .configuration_id(configuration_root);
        let direct_id =
            RuntimeProcessConfig::new(direct.clone(), policy).configuration_id(configuration_root);
        assert_eq!(
            wrapped_id, direct_id,
            "{label} changed configuration identity"
        );
        assert_eq!(
            wrapped.status_cache_state_path(),
            direct.layout.state_file,
            "{label} changed the observational retrieval state path"
        );
        assert_structurally_equal(wrapped.as_inner(), &direct);
        assert_structurally_equal(&wrapped.into_inner(), &direct);
    }

    #[test]
    fn constructors_preserve_retrieval_configuration_identity_and_structure() {
        let fixture = tempdir().expect("runtime retrieval fixture");
        let project_root = fixture.path().join("project");
        let cache_root = fixture.path().join("cache");
        std::fs::create_dir_all(&project_root).expect("create project fixture");
        let process_defaults =
            RetrievalProcessDefaults::new(cache_root, RetrievalRuntimeDefaults::default());
        let overrides = RetrievalRuntimeOverrides {
            hybrid_retrieval_enabled: Some(false),
            semantic_doc_scope: Some("all".to_string()),
            semantic_doc_alias_mode: Some("canonical".to_string()),
            summary_endpoint: Some("http://127.0.0.1:1".to_string()),
            summary_model: Some("identity-fixture".to_string()),
        };
        let configuration_root = fixture.path().join("project-cache");

        assert_constructor_pair(
            "local",
            RuntimeRetrievalConfig::local(),
            codestory_retrieval::SidecarRuntimeConfig::local(),
            &configuration_root,
        );
        assert_constructor_pair(
            "project auto",
            RuntimeRetrievalConfig::for_project_auto(&project_root),
            codestory_retrieval::SidecarRuntimeConfig::for_project_auto(&project_root),
            &configuration_root,
        );
        assert_constructor_pair(
            "project auto with process defaults",
            RuntimeRetrievalConfig::for_project_auto_with_process_defaults(
                &project_root,
                &process_defaults,
                &overrides,
            ),
            codestory_retrieval::SidecarRuntimeConfig::for_project_auto_with_process_defaults(
                &project_root,
                &process_defaults,
                &overrides,
            ),
            &configuration_root,
        );

        let project_identity = codestory_workspace::project_identity_v3(&project_root);
        assert_constructor_pair(
            "project auto with retained identity",
            RuntimeRetrievalConfig::for_project_auto_with_process_defaults_and_identity(
                &project_identity,
                &process_defaults,
                &overrides,
            ),
            codestory_retrieval::SidecarRuntimeConfig::for_project_auto_with_process_defaults_and_identity(
                &project_identity,
                &process_defaults,
                &overrides,
            ),
            &configuration_root,
        );

        for (label, profile, direct_profile) in [
            (
                "local profile",
                RuntimeRetrievalProfile::Local,
                codestory_retrieval::SidecarProfile::Local,
            ),
            (
                "agent profile",
                RuntimeRetrievalProfile::Agent,
                codestory_retrieval::SidecarProfile::Agent,
            ),
        ] {
            assert_constructor_pair(
                label,
                RuntimeRetrievalConfig::for_project_profile(Some(&project_root), profile),
                codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                    Some(&project_root),
                    direct_profile,
                ),
                &configuration_root,
            );
            for run_id in [None, Some("identity-run")] {
                assert_constructor_pair(
                    &format!("{label} with run id {run_id:?}"),
                    RuntimeRetrievalConfig::for_project_profile_with_run_id(
                        Some(&project_root),
                        profile,
                        run_id,
                    ),
                    codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_run_id(
                        Some(&project_root),
                        direct_profile,
                        run_id,
                    ),
                    &configuration_root,
                );
                assert_constructor_pair(
                    &format!("{label} with process defaults and run id {run_id:?}"),
                    RuntimeRetrievalConfig::for_project_profile_with_process_defaults(
                        Some(&project_root),
                        profile,
                        run_id,
                        &process_defaults,
                        &overrides,
                    ),
                    codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_process_defaults(
                        Some(&project_root),
                        direct_profile,
                        run_id,
                        &process_defaults,
                        &overrides,
                    ),
                    &configuration_root,
                );
            }
        }

        let wrapped_base = RuntimeRetrievalConfig::for_project_profile_with_process_defaults(
            Some(&project_root),
            RuntimeRetrievalProfile::Local,
            None,
            &process_defaults,
            &overrides,
        );
        let direct_base =
            codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_process_defaults(
                Some(&project_root),
                codestory_retrieval::SidecarProfile::Local,
                None,
                &process_defaults,
                &overrides,
            );
        for (profile, direct_profile) in [
            (
                RuntimeRetrievalProfile::Local,
                codestory_retrieval::SidecarProfile::Local,
            ),
            (
                RuntimeRetrievalProfile::Agent,
                codestory_retrieval::SidecarProfile::Agent,
            ),
        ] {
            for run_id in [None, Some("reselected")] {
                assert_constructor_pair(
                    &format!("profile reselection {profile:?} with run id {run_id:?}"),
                    wrapped_base.with_profile_and_run_id(Some(&project_root), profile, run_id),
                    direct_base.with_profile_and_run_id(
                        Some(&project_root),
                        direct_profile,
                        run_id,
                    ),
                    &configuration_root,
                );
            }
        }
    }

    #[test]
    fn profile_wrapper_preserves_both_profile_variants() {
        for (wrapped, direct, label) in [
            (
                RuntimeRetrievalProfile::Local,
                codestory_retrieval::SidecarProfile::Local,
                "local",
            ),
            (
                RuntimeRetrievalProfile::Agent,
                codestory_retrieval::SidecarProfile::Agent,
                "agent",
            ),
        ] {
            assert_eq!(codestory_retrieval::SidecarProfile::from(wrapped), direct);
            assert_eq!(RuntimeRetrievalProfile::from(direct), wrapped);
            assert_eq!(wrapped.as_str(), label);
        }
    }

    fn runtime_config_for_cache(
        project_root: &Path,
        cache_root: &Path,
        profile: RuntimeRetrievalProfile,
        run_id: Option<&str>,
    ) -> RuntimeRetrievalConfig {
        let defaults = RetrievalProcessDefaults::new(
            cache_root.to_path_buf(),
            RetrievalRuntimeDefaults::default(),
        );
        RuntimeRetrievalConfig::for_project_profile_with_process_defaults(
            Some(project_root),
            profile,
            run_id,
            &defaults,
            &RetrievalRuntimeOverrides::default(),
        )
    }

    fn error_chain(error: &anyhow::Error) -> Vec<String> {
        error.chain().map(ToString::to_string).collect()
    }

    #[test]
    fn embedding_preflight_preserves_selected_configuration_and_error_chain() {
        let fixture = tempdir().expect("runtime embedding preflight fixture");
        let available_root = fixture.path().join("available");
        let unavailable_root = fixture.path().join("unavailable");
        std::fs::create_dir_all(&available_root).expect("create available embedding root");
        std::fs::create_dir_all(&unavailable_root).expect("create unavailable embedding root");
        std::fs::write(
            unavailable_root.join(codestory_retrieval::TEST_EMBEDDING_UNAVAILABLE_MARKER),
            b"unavailable",
        )
        .expect("write embedding unavailable marker");

        for (profile, run_id) in [
            (RuntimeRetrievalProfile::Local, None),
            (RuntimeRetrievalProfile::Agent, Some("selected-agent-run")),
        ] {
            let available =
                runtime_config_for_cache(fixture.path(), &available_root, profile, run_id);
            codestory_retrieval::with_test_cache_root(&unavailable_root, || {
                codestory_retrieval::ensure_product_embedding_backend_for_runtime(
                    available.as_inner(),
                )
                .expect("owner preflight must use the selected available root");
                ensure_product_embedding_backend_for_runtime(&available)
                    .expect("runtime preflight must use the selected available root");
            });

            let unavailable =
                runtime_config_for_cache(fixture.path(), &unavailable_root, profile, run_id);
            codestory_retrieval::with_test_cache_root(&available_root, || {
                let direct = codestory_retrieval::ensure_product_embedding_backend_for_runtime(
                    unavailable.as_inner(),
                )
                .expect_err("owner preflight must use the selected unavailable root");
                let facade = ensure_product_embedding_backend_for_runtime(&unavailable)
                    .expect_err("runtime preflight must use the selected unavailable root");
                assert_eq!(error_chain(&facade), error_chain(&direct));
                assert_eq!(facade.to_string(), direct.to_string());
                assert!(
                    facade
                        .to_string()
                        .contains(&unavailable_root.display().to_string())
                );
            });
        }
    }

    fn directory_listing(root: &Path) -> BTreeMap<String, String> {
        fn collect(root: &Path, current: &Path, entries: &mut BTreeMap<String, String>) {
            let mut paths = std::fs::read_dir(current)
                .expect("read cleanup listing")
                .map(|entry| entry.expect("read cleanup entry").path())
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                let relative = path
                    .strip_prefix(root)
                    .expect("cleanup entry below root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let metadata = std::fs::symlink_metadata(&path).expect("cleanup metadata");
                if metadata.is_dir() {
                    entries.insert(format!("{relative}/"), "directory".into());
                    collect(root, &path, entries);
                } else {
                    entries.insert(relative, format!("file: {} bytes", metadata.len()));
                }
            }
        }

        let mut entries = BTreeMap::new();
        collect(root, root, &mut entries);
        entries
    }

    #[test]
    fn cache_clean_functions_preserve_owner_plan_report_and_post_state() {
        let fixture = tempdir().expect("runtime cache-clean fixture");
        let cache_root = fixture.path().join("cache");
        let worktrees_root = fixture.path().join("worktrees");
        codestory_retrieval::test_support::populate_cache_clean_fixture(
            &cache_root,
            &worktrees_root,
        )
        .expect("populate direct cache-clean fixture");

        let (direct_plan, facade_plan, direct_report) =
            codestory_retrieval::with_test_cache_root(&cache_root, || {
                let direct_plan =
                    codestory_retrieval::plan_cache_clean().expect("owner cleanup plan");
                let facade_plan = plan_cache_clean().expect("runtime cleanup plan");
                let direct_report =
                    codestory_retrieval::apply_cache_clean().expect("owner cleanup apply");
                (direct_plan, facade_plan, direct_report)
            });
        assert_eq!(facade_plan, direct_plan);
        assert_eq!(
            direct_plan.candidates.len(),
            2,
            "both proof classes must fire"
        );
        assert_eq!(
            direct_report.removals.len(),
            2,
            "both proof classes must apply"
        );
        let direct_post_state = directory_listing(&cache_root);

        std::fs::remove_dir_all(&cache_root).expect("reset cache-clean fixture root");
        std::fs::remove_dir_all(&worktrees_root).expect("reset cache-clean worktrees root");
        codestory_retrieval::test_support::populate_cache_clean_fixture(
            &cache_root,
            &worktrees_root,
        )
        .expect("repopulate runtime cache-clean fixture");
        let facade_report = codestory_retrieval::with_test_cache_root(&cache_root, || {
            apply_cache_clean().expect("runtime cleanup apply")
        });

        assert_eq!(facade_report, direct_report);
        assert_eq!(directory_listing(&cache_root), direct_post_state);
    }
}
