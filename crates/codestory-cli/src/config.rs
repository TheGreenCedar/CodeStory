use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CliConfig {
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) embedding_profile: Option<String>,
    pub(crate) embedding_model_id: Option<String>,
    #[serde(default)]
    pub(crate) embedding_model: Option<String>,
    pub(crate) hybrid_retrieval_enabled: Option<bool>,
    pub(crate) semantic_doc_scope: Option<String>,
    pub(crate) semantic_doc_alias_mode: Option<String>,
    pub(crate) summary_endpoint: Option<String>,
    pub(crate) summary_model: Option<String>,
}

pub(crate) fn load_config(project_root: &Path) -> Result<CliConfig> {
    let mut config = CliConfig::default();
    if let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    {
        merge_config_file(&mut config, &home.join(".codestory.toml"))?;
    }
    merge_config_file(&mut config, &project_root.join(".codestory.toml"))?;
    apply_env_defaults(&config);
    Ok(config)
}

fn merge_config_file(config: &mut CliConfig, path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config {}", path.display()))?;
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

fn apply_env_defaults(config: &CliConfig) {
    set_env_if_absent(
        "CODESTORY_EMBED_PROFILE",
        config.embedding_profile.as_deref(),
    );
    set_env_if_absent(
        "CODESTORY_EMBED_MODEL_ID",
        config.embedding_model_id.as_deref(),
    );
    set_env_if_absent(
        "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
        config
            .hybrid_retrieval_enabled
            .map(|value| if value { "true" } else { "false" }),
    );
    set_env_if_absent(
        "CODESTORY_SEMANTIC_DOC_SCOPE",
        config.semantic_doc_scope.as_deref(),
    );
    set_env_if_absent(
        "CODESTORY_SEMANTIC_DOC_ALIAS_MODE",
        config.semantic_doc_alias_mode.as_deref(),
    );
    set_env_if_absent(
        "CODESTORY_SUMMARY_ENDPOINT",
        config.summary_endpoint.as_deref(),
    );
    set_env_if_absent("CODESTORY_SUMMARY_MODEL", config.summary_model.as_deref());
}

fn set_env_if_absent(name: &str, value: Option<&str>) {
    if std::env::var_os(name).is_some() {
        return;
    }
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        unsafe {
            std::env::set_var(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::tempdir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        _lock: MutexGuard<'static, ()>,
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .expect("env test lock should not be poisoned");
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
    fn config_sets_embedding_profile_and_model_id_runtime_env_defaults() -> Result<()> {
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
        assert_eq!(
            std::env::var("CODESTORY_EMBED_PROFILE").as_deref(),
            Ok("bge-small-en-v1.5")
        );
        assert_eq!(
            std::env::var("CODESTORY_EMBED_MODEL_ID").as_deref(),
            Ok("BAAI/bge-small-en-v1.5-local")
        );
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
        assert_eq!(
            std::env::var("CODESTORY_EMBED_MODEL_ID").as_deref(),
            Ok("legacy/model-id")
        );
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
        assert_eq!(
            std::env::var("CODESTORY_EMBED_MODEL_ID").as_deref(),
            Ok("current/model-id")
        );

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
        assert_eq!(
            std::env::var("CODESTORY_EMBED_PROFILE").as_deref(),
            Ok("project-profile")
        );
        assert_eq!(
            std::env::var("CODESTORY_EMBED_MODEL_ID").as_deref(),
            Ok("project/model-id")
        );

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
        assert_eq!(
            std::env::var("CODESTORY_EMBED_MODEL_ID").as_deref(),
            Ok("project/legacy-model-id")
        );
        assert!(std::env::var_os("CODESTORY_EMBEDDING_MODEL").is_none());

        Ok(())
    }
}
