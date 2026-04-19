use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CliConfig {
    pub(crate) cache_dir: Option<PathBuf>,
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
    if file_config.embedding_model.is_some() {
        config.embedding_model = file_config.embedding_model;
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
        "CODESTORY_EMBEDDING_MODEL",
        config.embedding_model.as_deref(),
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
