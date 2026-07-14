use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::OnceLock;

const PROJECT_NETWORK_CONFIG_OPT_IN_ENV: &str = "CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG";

#[derive(Debug, Clone)]
pub(crate) struct CliStartupConfig {
    pub(crate) user_home: Option<PathBuf>,
    pub(crate) project_network_config_allowed: bool,
    pub(crate) stdio_cache_root: Option<PathBuf>,
    pub(crate) sidecar_defaults: codestory_retrieval::SidecarProcessDefaults,
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
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CliConfig {
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) embedding_profile: Option<String>,
    pub(crate) embedding_model_id: Option<String>,
    #[serde(default)]
    pub(crate) embedding_model: Option<String>,
    pub(crate) embedding_endpoint: Option<String>,
    pub(crate) embedding_query_prefix: Option<String>,
    pub(crate) embedding_document_prefix: Option<String>,
    pub(crate) hybrid_retrieval_enabled: Option<bool>,
    pub(crate) semantic_doc_scope: Option<String>,
    pub(crate) semantic_doc_alias_mode: Option<String>,
    pub(crate) summary_endpoint: Option<String>,
    pub(crate) summary_model: Option<String>,
    #[serde(skip)]
    embedding_endpoint_source: Option<ConfigSource>,
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
    if file_config.embedding_profile.is_some() {
        config.embedding_profile = file_config.embedding_profile;
    }
    if file_config.embedding_model_id.is_some() || file_config.embedding_model.is_some() {
        config.embedding_model_id = file_config
            .embedding_model_id
            .or(file_config.embedding_model);
    }
    if file_config.embedding_endpoint.is_some() {
        config.embedding_endpoint = file_config.embedding_endpoint;
        config.embedding_endpoint_source = Some(source);
    }
    if file_config.embedding_query_prefix.is_some() {
        config.embedding_query_prefix = file_config.embedding_query_prefix;
    }
    if file_config.embedding_document_prefix.is_some() {
        config.embedding_document_prefix = file_config.embedding_document_prefix;
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
    for field in ["summary_endpoint", "summary_model", "embedding_endpoint"] {
        if table.contains_key(field) && !project_network_config_allowed {
            anyhow::bail!(
                "project config field `{field}` is not trusted; set CODESTORY_SUMMARY_ENDPOINT, CODESTORY_SUMMARY_MODEL, CODESTORY_EMBED_LLAMACPP_URL, or pass a trusted CLI option instead"
            );
        }
    }
    Ok(())
}

impl CliConfig {
    pub(crate) fn runtime_overrides(&self) -> codestory_retrieval::SidecarRuntimeOverrides {
        codestory_retrieval::SidecarRuntimeOverrides {
            embedding_profile: self.embedding_profile.clone(),
            embedding_model_id: self.embedding_model_id.clone(),
            embedding_endpoint: self.embedding_endpoint.clone(),
            embedding_endpoint_origin: self.embedding_endpoint_source.map(|source| match source {
                ConfigSource::TrustedUser => {
                    codestory_retrieval::EmbeddingEndpointOrigin::TrustedUserConfig
                }
                ConfigSource::Project => {
                    codestory_retrieval::EmbeddingEndpointOrigin::TrustedProjectConfig
                }
            }),
            embedding_query_prefix: self.embedding_query_prefix.clone(),
            embedding_document_prefix: self.embedding_document_prefix.clone(),
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
    fn config_retains_embedding_profile_and_model_without_mutating_environment() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_PROFILE",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);
        clear_env(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_PROFILE",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"
embedding_profile = "bge-small-en-v1.5"
embedding_model_id = "BAAI/bge-small-en-v1.5-local"
"#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.embedding_profile.as_deref(),
            Some("bge-small-en-v1.5")
        );
        assert_eq!(
            config.embedding_model_id.as_deref(),
            Some("BAAI/bge-small-en-v1.5-local")
        );
        assert!(std::env::var("CODESTORY_EMBED_PROFILE").is_err());
        assert!(std::env::var("CODESTORY_EMBED_MODEL_ID").is_err());
        assert!(std::env::var_os("CODESTORY_EMBEDDING_MODEL").is_none());

        Ok(())
    }

    #[test]
    fn config_keeps_legacy_embedding_model_as_model_id_alias() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);
        clear_env(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"embedding_model = "legacy/model-id""#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.embedding_model_id.as_deref(),
            Some("legacy/model-id")
        );
        assert!(std::env::var("CODESTORY_EMBED_MODEL_ID").is_err());
        assert!(std::env::var_os("CODESTORY_EMBEDDING_MODEL").is_none());

        Ok(())
    }

    #[test]
    fn config_prefers_embedding_model_id_over_legacy_alias_in_same_file() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);
        clear_env(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"
embedding_model = "legacy/model-id"
embedding_model_id = "current/model-id"
"#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.embedding_model_id.as_deref(),
            Some("current/model-id")
        );
        assert!(std::env::var("CODESTORY_EMBED_MODEL_ID").is_err());

        Ok(())
    }

    #[test]
    fn config_preserves_explicit_embedding_env_over_config_defaults() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_PROFILE",
            "CODESTORY_EMBED_MODEL_ID",
        ]);
        clear_env(&["USERPROFILE", "HOME"]);
        unsafe {
            std::env::set_var("CODESTORY_EMBED_PROFILE", "explicit-profile");
            std::env::set_var("CODESTORY_EMBED_MODEL_ID", "explicit/model-id");
        }

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"
embedding_profile = "config-profile"
embedding_model_id = "config/model-id"
"#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(config.embedding_profile.as_deref(), Some("config-profile"));
        assert_eq!(
            config.embedding_model_id.as_deref(),
            Some("config/model-id")
        );
        assert_eq!(
            std::env::var("CODESTORY_EMBED_PROFILE").as_deref(),
            Ok("explicit-profile")
        );
        assert_eq!(
            std::env::var("CODESTORY_EMBED_MODEL_ID").as_deref(),
            Ok("explicit/model-id")
        );

        Ok(())
    }

    #[test]
    fn config_project_file_overrides_home_file() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_PROFILE",
            "CODESTORY_EMBED_MODEL_ID",
        ]);
        clear_env(&[
            "HOME",
            "CODESTORY_EMBED_PROFILE",
            "CODESTORY_EMBED_MODEL_ID",
        ]);

        let home = tempdir()?;
        let project = tempdir()?;
        unsafe {
            std::env::set_var("USERPROFILE", home.path());
        }
        std::fs::write(
            home.path().join(".codestory.toml"),
            r#"
embedding_profile = "home-profile"
embedding_model_id = "home/model-id"
"#,
        )?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"
embedding_profile = "project-profile"
embedding_model_id = "project/model-id"
"#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(config.embedding_profile.as_deref(), Some("project-profile"));
        assert_eq!(
            config.embedding_model_id.as_deref(),
            Some("project/model-id")
        );
        assert!(std::env::var("CODESTORY_EMBED_PROFILE").is_err());
        assert!(std::env::var("CODESTORY_EMBED_MODEL_ID").is_err());

        Ok(())
    }

    #[test]
    fn config_project_legacy_embedding_model_overrides_home_model_id() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);
        clear_env(&[
            "HOME",
            "CODESTORY_EMBED_MODEL_ID",
            "CODESTORY_EMBEDDING_MODEL",
        ]);

        let home = tempdir()?;
        let project = tempdir()?;
        unsafe {
            std::env::set_var("USERPROFILE", home.path());
        }
        std::fs::write(
            home.path().join(".codestory.toml"),
            r#"embedding_model_id = "home/current-model-id""#,
        )?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"embedding_model = "project/legacy-model-id""#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.embedding_model_id.as_deref(),
            Some("project/legacy-model-id")
        );
        assert!(std::env::var("CODESTORY_EMBED_MODEL_ID").is_err());
        assert!(std::env::var_os("CODESTORY_EMBEDDING_MODEL").is_none());

        Ok(())
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
    fn project_config_rejects_embedding_endpoint_without_trusted_opt_in() -> Result<()> {
        let _env = EnvRestore::capture(&["USERPROFILE", "HOME", PROJECT_NETWORK_CONFIG_OPT_IN_ENV]);
        clear_env(&["USERPROFILE", "HOME", PROJECT_NETWORK_CONFIG_OPT_IN_ENV]);

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"embedding_endpoint = "http://127.0.0.1:8080/v1/embeddings""#,
        )?;

        let err = load_config(project.path()).expect_err("project embedding endpoint should fail");
        let message = format!("{err:#}");
        assert!(message.contains("project config field `embedding_endpoint` is not trusted"));
        assert!(message.contains("CODESTORY_EMBED_LLAMACPP_URL"));

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
    fn trusted_opt_in_allows_project_embedding_endpoint() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            PROJECT_NETWORK_CONFIG_OPT_IN_ENV,
            "CODESTORY_EMBED_LLAMACPP_URL",
        ]);
        clear_env(&["USERPROFILE", "HOME", "CODESTORY_EMBED_LLAMACPP_URL"]);
        unsafe {
            std::env::set_var(PROJECT_NETWORK_CONFIG_OPT_IN_ENV, "1");
        }

        let project = tempdir()?;
        std::fs::write(
            project.path().join(".codestory.toml"),
            r#"embedding_endpoint = "http://127.0.0.1:8080/v1/embeddings""#,
        )?;

        let config = load_config(project.path())?;

        assert_eq!(
            config.embedding_endpoint.as_deref(),
            Some("http://127.0.0.1:8080/v1/embeddings")
        );
        assert!(std::env::var("CODESTORY_EMBED_LLAMACPP_URL").is_err());

        Ok(())
    }

    #[test]
    fn home_config_can_set_cache_dir_and_network_defaults() -> Result<()> {
        let _env = EnvRestore::capture(&[
            "USERPROFILE",
            "HOME",
            "CODESTORY_SUMMARY_ENDPOINT",
            "CODESTORY_SUMMARY_MODEL",
            "CODESTORY_EMBED_LLAMACPP_URL",
        ]);
        clear_env(&[
            "HOME",
            "CODESTORY_SUMMARY_ENDPOINT",
            "CODESTORY_SUMMARY_MODEL",
            "CODESTORY_EMBED_LLAMACPP_URL",
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
embedding_endpoint = "http://127.0.0.1:8080/v1/embeddings"
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
        assert_eq!(
            config.embedding_endpoint.as_deref(),
            Some("http://127.0.0.1:8080/v1/embeddings")
        );
        assert!(std::env::var("CODESTORY_SUMMARY_ENDPOINT").is_err());
        assert!(std::env::var("CODESTORY_SUMMARY_MODEL").is_err());
        assert!(std::env::var("CODESTORY_EMBED_LLAMACPP_URL").is_err());

        Ok(())
    }
}
