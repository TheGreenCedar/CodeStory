//! CLI-owned construction boundary for retrieval sidecar runtimes.
//!
//! Keep every constructor that can consult the process cache root in this
//! module. Unit-test binaries enable their process-wide, named-thread cache
//! isolation before the constructor (or startup configuration) can fall back
//! to the platform `ProjectDirs` cache.

#[cfg(test)]
use codestory_retrieval::SidecarRuntimeDefaults;
use codestory_retrieval::{
    SidecarProcessDefaults, SidecarProfile, SidecarRuntimeConfig, SidecarRuntimeOverrides,
};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

/// Prepare process cache access before any CLI startup or sidecar lookup.
pub(crate) fn prepare_cache_access() {
    #[cfg(test)]
    codestory_retrieval::enable_automatic_test_cache_root_for_process();
}

#[cfg(test)]
fn with_default_test_cache_root<T>(task: impl FnOnce() -> T) -> T {
    prepare_cache_access();
    let root = codestory_retrieval::active_test_cache_root().unwrap_or_else(|| {
        static PROCESS_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        PROCESS_ROOT
            .get_or_init(|| {
                let nonce = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let root = std::env::temp_dir()
                    .join("codestory-cli-unit-tests")
                    .join(format!("{}-{nonce}", std::process::id()));
                std::fs::create_dir_all(root.join("sidecars"))
                    .expect("create CLI unit-test cache root");
                root
            })
            .clone()
    });
    codestory_retrieval::with_test_cache_root(&root, task)
}

pub(crate) fn process_defaults() -> SidecarProcessDefaults {
    prepare_cache_access();
    #[cfg(test)]
    return with_default_test_cache_root(|| {
        SidecarProcessDefaults::new(
            codestory_retrieval::user_cache_root(),
            SidecarRuntimeDefaults::from_process_env(),
        )
    });
    #[cfg(not(test))]
    codestory_retrieval::sidecar_process_defaults()
}

#[cfg(test)]
pub(crate) fn user_cache_root() -> PathBuf {
    process_defaults().cache_root().to_path_buf()
}

pub(crate) fn local() -> SidecarRuntimeConfig {
    let defaults = process_defaults();
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        None,
        SidecarProfile::Local,
        None,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    )
}

#[cfg(test)]
pub(crate) fn embedding_runtime_id() -> String {
    codestory_retrieval::embedding_runtime_id_for_runtime(&local())
}

pub(crate) fn for_project(project_root: &Path, profile: SidecarProfile) -> SidecarRuntimeConfig {
    let defaults = process_defaults();
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        Some(project_root),
        profile,
        None,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    )
}

pub(crate) fn for_project_with_run_id(
    project_root: &Path,
    profile: SidecarProfile,
    run_id: Option<&str>,
) -> SidecarRuntimeConfig {
    let defaults = process_defaults();
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        Some(project_root),
        profile,
        run_id,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    )
}

pub(crate) fn for_project_auto_with_process_defaults(
    project_root: &Path,
    defaults: &SidecarProcessDefaults,
    overrides: &SidecarRuntimeOverrides,
) -> SidecarRuntimeConfig {
    prepare_cache_access();
    SidecarRuntimeConfig::for_project_auto_with_process_defaults(project_root, defaults, overrides)
}

#[cfg(test)]
pub(crate) fn spawn_with_cache_access<T: Send + 'static>(
    task: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    let root = user_cache_root();
    std::thread::spawn(move || codestory_retrieval::with_test_cache_root(&root, task))
}

#[cfg(test)]
pub(crate) fn for_project_with_run_id_in_cache(
    project_root: Option<&Path>,
    profile: SidecarProfile,
    run_id: Option<&str>,
    cache_root: &Path,
) -> SidecarRuntimeConfig {
    prepare_cache_access();
    let defaults = SidecarProcessDefaults::new(
        cache_root.to_path_buf(),
        SidecarRuntimeDefaults::from_process_env(),
    );
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        project_root,
        profile,
        run_id,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_activates_isolation_before_default_cache_lookup() {
        let cache_root = user_cache_root();
        let active_root = codestory_retrieval::active_test_cache_root()
            .expect("gateway should activate a process-unique test cache root");

        assert_eq!(cache_root, active_root);
        assert!(local().layout.state_file.starts_with(&cache_root));
    }

    #[test]
    fn named_test_threads_get_stable_distinct_cache_roots() {
        if std::env::var_os("CODESTORY_CACHE_ROOT").is_some_and(|value| !value.is_empty()) {
            return;
        }
        let roots = ["sidecar-cache-a", "sidecar-cache-b"].map(|name| {
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(|| {
                    let first = user_cache_root();
                    let second = user_cache_root();
                    assert_eq!(first, second);
                    assert!(local().layout.state_file.starts_with(&first));
                    first
                })
                .expect("spawn named cache test thread")
        });
        let [first, second] = roots.map(|thread| thread.join().expect("cache test thread"));

        assert_ne!(first, second);
    }

    #[test]
    fn unnamed_test_workers_never_fall_back_to_platform_cache() {
        if std::env::var_os("CODESTORY_CACHE_ROOT").is_some_and(|value| !value.is_empty()) {
            return;
        }
        let (root, state_file) = std::thread::spawn(|| {
            let root = user_cache_root();
            let state_file = local().layout.state_file;
            (root, state_file)
        })
        .join()
        .expect("unnamed cache test thread");

        assert!(root.starts_with(std::env::temp_dir().join("codestory-cli-unit-tests")));
        assert!(state_file.starts_with(root));
    }

    #[test]
    fn gateway_workers_inherit_the_calling_test_namespace() {
        let parent = user_cache_root();
        let child = spawn_with_cache_access(user_cache_root)
            .join()
            .expect("cache-aware worker");

        assert_eq!(child, parent);
    }
}
