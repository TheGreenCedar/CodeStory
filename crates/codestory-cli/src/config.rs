use anyhow::{Context, Result};
use codestory_contracts::workspace::SourceIndexPolicy;
use serde::Deserialize;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::OnceLock;

const PROJECT_NETWORK_CONFIG_OPT_IN_ENV: &str = "CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG";
const SOURCE_FILE_BYTE_CAP_ENV: &str = "CODESTORY_INDEX_SOURCE_FILE_BYTE_CAP";

#[derive(Debug, Clone)]
pub(crate) struct CliStartupConfig {
    pub(crate) user_home: Option<PathBuf>,
    pub(crate) project_network_config_allowed: bool,
    pub(crate) stdio_cache_root: Option<PathBuf>,
    pub(crate) sidecar_defaults: codestory_retrieval::SidecarProcessDefaults,
    pub(crate) source_index_policy: SourceIndexPolicy,
}

impl CliStartupConfig {
    pub(crate) fn from_process_env() -> Self {
        crate::sidecar_runtime::prepare_cache_access();
        Self {
            user_home: std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from),
            project_network_config_allowed: std::env::var(PROJECT_NETWORK_CONFIG_OPT_IN_ENV)
                .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
                .unwrap_or(false),
            stdio_cache_root: std::env::var_os("CODESTORY_STDIO_CACHE_ROOT").map(PathBuf::from),
            sidecar_defaults: crate::sidecar_runtime::process_defaults(),
            source_index_policy: source_index_policy_from_env_value(
                std::env::var(SOURCE_FILE_BYTE_CAP_ENV).ok().as_deref(),
            ),
        }
    }
}

fn source_index_policy_from_env_value(raw: Option<&str>) -> SourceIndexPolicy {
    let byte_cap = raw
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|cap| *cap > 0)
        .unwrap_or(codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP);
    SourceIndexPolicy::oversized(byte_cap)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CliConfig {
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) hybrid_retrieval_enabled: Option<bool>,
    pub(crate) semantic_doc_scope: Option<String>,
    pub(crate) semantic_doc_alias_mode: Option<String>,
    pub(crate) summary_endpoint: Option<String>,
    pub(crate) summary_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigSource {
    TrustedUser,
    Project,
}

#[cfg(test)]
pub(crate) fn load_config(project_root: &Path) -> Result<CliConfig> {
    load_config_with_startup(project_root, &process_startup_config())
}

pub(crate) fn load_config_with_startup(
    project_root: &Path,
    startup: &CliStartupConfig,
) -> Result<CliConfig> {
    let mut config = CliConfig::default();
    if let Some(home) = startup.user_home.as_ref() {
        merge_config_file(
            &mut config,
            &home.join(".codestory.toml"),
            ConfigSource::TrustedUser,
            startup.project_network_config_allowed,
        )?;
    }
    merge_config_file(
        &mut config,
        &project_root.join(".codestory.toml"),
        ConfigSource::Project,
        startup.project_network_config_allowed,
    )?;
    Ok(config)
}

pub(crate) fn process_startup_config() -> CliStartupConfig {
    #[cfg(test)]
    {
        CliStartupConfig::from_process_env()
    }
    #[cfg(not(test))]
    {
        static STARTUP: OnceLock<CliStartupConfig> = OnceLock::new();
        STARTUP
            .get_or_init(CliStartupConfig::from_process_env)
            .clone()
    }
}

fn merge_config_file(
    config: &mut CliConfig,
    path: &Path,
    source: ConfigSource,
    project_network_config_allowed: bool,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config {}", path.display()))?;
    validate_config_trust_boundary(&raw, source, path, project_network_config_allowed)?;
    let file_config: CliConfig = toml::from_str(&raw)
        .with_context(|| format!("Failed to parse config {}", path.display()))?;
    if file_config.cache_dir.is_some() {
        config.cache_dir = file_config.cache_dir;
    }
    if file_config.hybrid_retrieval_enabled.is_some() {
        config.hybrid_retrieval_enabled = file_config.hybrid_retrieval_enabled;
    }
    if file_config.semantic_doc_scope.is_some() {
        config.semantic_doc_scope = file_config.semantic_doc_scope;
    }
    if file_config.semantic_doc_alias_mode.is_some() {
        config.semantic_doc_alias_mode = file_config.semantic_doc_alias_mode;
    }
    if file_config.summary_endpoint.is_some() {
        config.summary_endpoint = file_config.summary_endpoint;
    }
    if file_config.summary_model.is_some() {
        config.summary_model = file_config.summary_model;
    }
    Ok(())
}

fn validate_config_trust_boundary(
    raw: &str,
    source: ConfigSource,
    path: &Path,
    project_network_config_allowed: bool,
) -> Result<()> {
    if source != ConfigSource::Project {
        return Ok(());
    }
    let value: toml::Value = toml::from_str(raw)
        .with_context(|| format!("Failed to parse config {}", path.display()))?;
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    if table.contains_key("cache_dir") {
        anyhow::bail!(
            "project config field `cache_dir` is not trusted; set it in the user home .codestory.toml or pass --cache-dir instead"
        );
    }
    for field in ["summary_endpoint", "summary_model"] {
        if table.contains_key(field) && !project_network_config_allowed {
            anyhow::bail!(
                "project config field `{field}` is not trusted; set CODESTORY_SUMMARY_ENDPOINT or CODESTORY_SUMMARY_MODEL, or pass a trusted CLI option instead"
            );
        }
    }
    Ok(())
}

impl CliConfig {
    pub(crate) fn runtime_overrides(&self) -> codestory_retrieval::SidecarRuntimeOverrides {
        codestory_retrieval::SidecarRuntimeOverrides {
            hybrid_retrieval_enabled: self.hybrid_retrieval_enabled,
            semantic_doc_scope: self.semantic_doc_scope.clone(),
            semantic_doc_alias_mode: self.semantic_doc_alias_mode.clone(),
            summary_endpoint: self.summary_endpoint.clone(),
            summary_model: self.summary_model.clone(),
        }
    }
}

#[cfg(test)]
static CONFIG_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn config_env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_ENV_TEST_LOCK
        .lock()
        // A failed assertion must not turn one environment-sensitive test into
        // a cascade of unrelated mutex-poison failures. Test snapshots restore
        // their variables during unwinding, so retaining the guard is safe.
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::tempdir;

    struct EnvRestore {
        _lock: std::sync::MutexGuard<'static, ()>,
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            let lock = config_env_test_lock();
            let values = names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
            Self {
                _lock: lock,
                values,
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in &self.values {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    fn clear_env(names: &[&str]) {
        for name in names {
            unsafe {
                std::env::remove_var(name);
            }
        }
    }

    #[test]
    fn source_index_policy_parses_only_positive_process_values() {
        assert_eq!(
            source_index_policy_from_env_value(Some(" 65536 ")).byte_cap,
            65_536
        );
        for raw in [None, Some(""), Some("0"), Some("-1"), Some("invalid")] {
            assert_eq!(
                source_index_policy_from_env_value(raw),
                SourceIndexPolicy::default()
            );
        }
    }

    #[test]
    fn project_config_rejects_cache_dir() -> Result<()> {
        let _env = EnvRestore::capture(&["USERPROFILE", "HOME"]);
        clear_env(&["USERPROFILE", "HOME"]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"cache_dir = "C:/repo-controlled-cache""#,
        )?;

        let err = load_config(project.path()).expect_err("project cache_dir should fail closed");
        let message = format!("{err:#}");
        assert!(message.contains("project config field `cache_dir` is not trusted"));
        assert!(message.contains("user home .codestory.toml"));
        assert!(message.contains("--cache-dir"));

        Ok(())
    }

    #[test]
    fn project_config_rejects_summary_endpoint_without_trusted_opt_in() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            PROJECT_NETWORK_CONFIG_OPT_IN_ENV,
            "CODESTORY_SUMMARY_ENDPOINT",
        ]);
        clear_env(&[
            "USERPROFILE",
            "HOME",
            PROJECT_NETWORK_CONFIG_OPT_IN_ENV,
            "CODESTORY_SUMMARY_ENDPOINT",
        ]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"summary_endpoint = "https://example.invalid/v1/chat/completions""#,
        )?;

        let err = load_config(project.path()).expect_err("project summary endpoint should fail");
        let message = format!("{err:#}");
        assert!(message.contains("project config field `summary_endpoint` is not trusted"));
        assert!(message.contains("CODESTORY_SUMMARY_ENDPOINT"));
        assert!(message.contains("trusted CLI option"));
        assert!(std::env::var_os("CODESTORY_SUMMARY_ENDPOINT").is_none());

        Ok(())
    }

    #[test]
    fn project_config_rejects_summary_model_without_trusted_opt_in() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            PROJECT_NETWORK_CONFIG_OPT_IN_ENV,
            "CODESTORY_SUMMARY_MODEL",
        ]);
        clear_env(&[
            "USERPROFILE",
            "HOME",
            PROJECT_NETWORK_CONFIG_OPT_IN_ENV,
            "CODESTORY_SUMMARY_MODEL",
        ]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"summary_model = "expensive/repo-selected-model""#,
        )?;

        let err = load_config(project.path()).expect_err("project summary model should fail");
        let message = format!("{err:#}");
        assert!(message.contains("project config field `summary_model` is not trusted"));
        assert!(message.contains("CODESTORY_SUMMARY_MODEL"));
        assert!(std::env::var_os("CODESTORY_SUMMARY_MODEL").is_none());

        Ok(())
    }

    #[test]
    fn trusted_opt_in_allows_project_summary_endpoint() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            PROJECT_NETWORK_CONFIG_OPT_IN_ENV,
            "CODESTORY_SUMMARY_ENDPOINT",
        ]);
        clear_env(&["USERPROFILE", "HOME", "CODESTORY_SUMMARY_ENDPOINT"]);
        unsafe {
            std::env::set_var(PROJECT_NETWORK_CONFIG_OPT_IN_ENV, "1");
        }

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"summary_endpoint = "https://example.invalid/v1/chat/completions""#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.summary_endpoint.as_deref(),
            Some("https://example.invalid/v1/chat/completions")
        );
        assert!(std::env::var("CODESTORY_SUMMARY_ENDPOINT").is_err());

        Ok(())
    }

    #[test]
    fn home_config_can_set_cache_dir_and_network_defaults() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_SUMMARY_ENDPOINT",
            "CODESTORY_SUMMARY_MODEL",
        ]);
        clear_env(&[
            "HOME",
            "CODESTORY_SUMMARY_ENDPOINT",
            "CODESTORY_SUMMARY_MODEL",
        ]);

        let home = tempdir()?;
        let project = tempdir()?;
        unsafe {
            std::env::set_var("USERPROFILE", home.path());
        }
        std::fs::write(
            home.path().join(".codestory.toml"),
            r#"
cache_dir = "C:/trusted-cache"
summary_endpoint = "https://example.invalid/v1/chat/completions"
summary_model = "trusted/model"
"#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.cache_dir.as_deref(),
            Some(Path::new("C:/trusted-cache"))
        );
        assert_eq!(
            config.summary_endpoint.as_deref(),
            Some("https://example.invalid/v1/chat/completions")
        );
        assert_eq!(config.summary_model.as_deref(), Some("trusted/model"));
        assert!(std::env::var("CODESTORY_SUMMARY_ENDPOINT").is_err());
        assert!(std::env::var("CODESTORY_SUMMARY_MODEL").is_err());

        Ok(())
    }
}
