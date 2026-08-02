use anyhow::{Context, Result};
use codestory_contracts::config_registry::{
    self, CONFIG_SCHEMA_VERSION_KEY, UNSUPPORTED_CONFIG_SCHEMA_CODE, UnknownKeyPolicy,
};
use codestory_contracts::workspace::SourceIndexPolicy;
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::OnceLock;

const PROJECT_NETWORK_CONFIG_OPT_IN_ENV: &str = config_registry::ALLOW_PROJECT_NETWORK_CONFIG_ENV;
const SOURCE_FILE_BYTE_CAP_ENV: &str = config_registry::INDEX_SOURCE_FILE_BYTE_CAP_ENV;

/// Prefix pairing with the `Error: ` line the CLI prints for fatal failures.
const CONFIG_WARNING_PREFIX: &str = "Warning: ";

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

/// Every honoured `.codestory.toml` key deserializes into one field here.
///
/// The test build also serializes it, which is how
/// `every_registered_config_key_is_read_into_a_field` binds
/// `config_registry::CONFIG_FILE_KEYS` to the fields that actually exist
/// instead of restating the registry back to itself.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
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
    load_config_with_startup(project_root, &process_startup_config(), &mut Vec::new())
}

/// Load configuration for one project and report its non-fatal problems.
///
/// `report_to` is the user-visible stream the caller reads: production passes
/// stderr. Schema warnings must land on a surface a person actually sees —
/// the process diagnostics sink is a private rotating file that redacts every
/// free-form field, so a warning routed only through `tracing` loses the very
/// key names that make it actionable.
pub(crate) fn load_config_with_startup(
    project_root: &Path,
    startup: &CliStartupConfig,
    report_to: &mut dyn Write,
) -> Result<CliConfig> {
    let (config, warnings) = load_config_report(project_root, startup)?;
    for warning in warnings {
        // A failed write to the report stream must not fail the command: the
        // configuration itself loaded.
        let _ = writeln!(report_to, "{CONFIG_WARNING_PREFIX}{warning}");
    }
    Ok(config)
}

/// Load configuration and return the schema warnings instead of reporting them.
///
/// Warnings name unknown keys only. The registry owns that text so no caller
/// can widen a warning into a value echo.
pub(crate) fn load_config_report(
    project_root: &Path,
    startup: &CliStartupConfig,
) -> Result<(CliConfig, Vec<String>)> {
    let mut config = CliConfig::default();
    let mut warnings = Vec::new();
    if let Some(home) = startup.user_home.as_ref() {
        merge_config_file(
            &mut config,
            &home.join(".codestory.toml"),
            ConfigSource::TrustedUser,
            startup.project_network_config_allowed,
            &mut warnings,
        )?;
    }
    merge_config_file(
        &mut config,
        &project_root.join(".codestory.toml"),
        ConfigSource::Project,
        startup.project_network_config_allowed,
        &mut warnings,
    )?;
    Ok((config, warnings))
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
    warnings: &mut Vec<String>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config {}", path.display()))?;
    let table = parse_config_table(&raw, path)?;
    enforce_config_schema(&table, path, warnings)?;
    validate_config_trust_boundary(&table, source, path, project_network_config_allowed)?;
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

fn parse_config_table(raw: &str, path: &Path) -> Result<toml::Table> {
    let value: toml::Value = toml::from_str(raw)
        .with_context(|| format!("Failed to parse config {}", path.display()))?;
    match value {
        toml::Value::Table(table) => Ok(table),
        _ => anyhow::bail!(
            "Failed to parse config {}: expected a table",
            path.display()
        ),
    }
}

/// Apply the registry's versioned unknown-key policy to one configuration file.
///
/// A declared version this build cannot interpret fails closed rather than
/// being read as the current schema, because a newer file can give a familiar
/// key a different meaning. These two failures are new shapes, so they carry
/// the registry's typed codes through the CLI's existing `ApiError` channel;
/// every failure this file already had keeps its previous generic envelope.
fn enforce_config_schema(
    table: &toml::Table,
    path: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let source_display = path.display().to_string();
    let version = declared_schema_version(table, &source_display)?;
    let policy = config_registry::unknown_key_policy(version)
        .map_err(|failure| typed_config_error(failure.to_api_error(&source_display)))?;
    let unknown = config_registry::unknown_config_keys(table.keys().map(String::as_str));
    if unknown.is_empty() {
        return Ok(());
    }
    match policy {
        UnknownKeyPolicy::Warn => {
            warnings.push(config_registry::unknown_key_warning(
                &source_display,
                &unknown,
            ));
            Ok(())
        }
        UnknownKeyPolicy::Reject => Err(typed_config_error(config_registry::unknown_key_error(
            &source_display,
            &unknown,
        ))),
    }
}

/// Carry a typed configuration failure on the same channel every other typed
/// CLI failure uses, so `codestory-cli --format json` reports its code.
fn typed_config_error(error: codestory_contracts::api::ApiError) -> anyhow::Error {
    crate::runtime::map_api_error(error)
}

fn declared_schema_version(table: &toml::Table, source_display: &str) -> Result<u64> {
    let Some(value) = table.get(CONFIG_SCHEMA_VERSION_KEY) else {
        return Ok(u64::from(config_registry::CONFIG_SCHEMA_CURRENT_VERSION));
    };
    match value.as_integer() {
        Some(version) if version >= 0 => Ok(version as u64),
        _ => Err(typed_config_error(codestory_contracts::api::ApiError::new(
            UNSUPPORTED_CONFIG_SCHEMA_CODE,
            format!(
                "{source_display} declares a {CONFIG_SCHEMA_VERSION_KEY} that is not a \
                 non-negative integer; this build supports at most {}",
                config_registry::CONFIG_SCHEMA_MAX_SUPPORTED_VERSION
            ),
        ))),
    }
}

fn validate_config_trust_boundary(
    table: &toml::Table,
    source: ConfigSource,
    _path: &Path,
    project_network_config_allowed: bool,
) -> Result<()> {
    if source != ConfigSource::Project {
        return Ok(());
    }
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
    use crate::runtime::api_error_in_chain;
    use anyhow::Result;
    use std::ffi::OsString;
    use tempfile::tempdir;

    /// Load through the production entry point and capture the stream a user
    /// reads, so a warning that never reaches a person fails the test.
    fn load_reporting_to_user(
        project_root: &Path,
        startup: &CliStartupConfig,
    ) -> Result<(CliConfig, String)> {
        let mut reported = Vec::new();
        let config = load_config_with_startup(project_root, startup, &mut reported)?;
        Ok((config, String::from_utf8(reported)?))
    }

    fn failure_code(error: &anyhow::Error) -> Option<&str> {
        api_error_in_chain(error).map(|error| error.code.as_str())
    }

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

    fn isolated_startup() -> CliStartupConfig {
        CliStartupConfig {
            user_home: None,
            project_network_config_allowed: false,
            stdio_cache_root: None,
            sidecar_defaults: crate::sidecar_runtime::process_defaults(),
            source_index_policy: SourceIndexPolicy::default(),
        }
    }

    fn write_project_config(project: &Path, body: &str) -> Result<()> {
        std::fs::write(project.join(".codestory.toml"), body)?;
        Ok(())
    }

    #[test]
    fn schema_v1_reports_unknown_keys_to_the_user_without_their_values() -> Result<()> {
        let project = tempdir()?;
        write_project_config(
            project.path(),
            "hybrid_retrieval_enabled = true\nembedding_query_prefix = \"secret-prefix\"\n",
        )?;

        let (config, reported) = load_reporting_to_user(project.path(), &isolated_startup())?;

        assert_eq!(config.hybrid_retrieval_enabled, Some(true));
        assert_eq!(
            reported.lines().count(),
            1,
            "one reported line per file with unknown keys: {reported:?}"
        );
        assert!(
            reported.starts_with(CONFIG_WARNING_PREFIX),
            "the user-visible line must read as a warning: {reported:?}"
        );
        assert!(
            reported.contains("embedding_query_prefix"),
            "the key name is the actionable part of the warning: {reported:?}"
        );
        assert!(
            !reported.contains("secret-prefix"),
            "warning must never echo a configured value: {reported:?}"
        );

        Ok(())
    }

    #[test]
    fn a_fully_registered_file_reports_nothing_to_the_user() -> Result<()> {
        let project = tempdir()?;
        write_project_config(project.path(), "hybrid_retrieval_enabled = true\n")?;

        let (_, reported) = load_reporting_to_user(project.path(), &isolated_startup())?;

        assert!(
            reported.is_empty(),
            "a clean configuration must stay silent: {reported:?}"
        );

        Ok(())
    }

    #[test]
    fn schema_v2_rejects_unknown_keys_with_a_typed_error() -> Result<()> {
        let project = tempdir()?;
        write_project_config(
            project.path(),
            "schema_version = 2\nembedding_query_prefix = \"secret-prefix\"\n",
        )?;

        let error = load_reporting_to_user(project.path(), &isolated_startup())
            .expect_err("schema version 2 rejects unknown keys");

        assert_eq!(failure_code(&error), Some("unknown_config_key"));
        let message = format!("{error:#}");
        assert!(message.contains("embedding_query_prefix"));
        assert!(
            !message.contains("secret-prefix"),
            "rejection must never echo a configured value: {message}"
        );

        Ok(())
    }

    #[test]
    fn schema_v2_accepts_a_file_whose_keys_are_all_registered() -> Result<()> {
        let project = tempdir()?;
        write_project_config(
            project.path(),
            "schema_version = 2\nhybrid_retrieval_enabled = false\n",
        )?;

        let (config, reported) = load_reporting_to_user(project.path(), &isolated_startup())?;

        assert_eq!(config.hybrid_retrieval_enabled, Some(false));
        assert!(reported.is_empty());

        Ok(())
    }

    #[test]
    fn future_schema_versions_fail_unsupported_config_schema() -> Result<()> {
        let project = tempdir()?;
        write_project_config(
            project.path(),
            "schema_version = 3\nhybrid_retrieval_enabled = true\n",
        )?;

        let error = load_reporting_to_user(project.path(), &isolated_startup())
            .expect_err("a future schema is not interpreted");

        assert_eq!(failure_code(&error), Some(UNSUPPORTED_CONFIG_SCHEMA_CODE));
        assert!(format!("{error:#}").contains("schema_version"));

        Ok(())
    }

    #[test]
    fn malformed_schema_versions_fail_closed() -> Result<()> {
        let project = tempdir()?;
        write_project_config(project.path(), "schema_version = \"one\"\n")?;

        let error = load_reporting_to_user(project.path(), &isolated_startup())
            .expect_err("a non-integer schema version is not interpreted");

        assert_eq!(failure_code(&error), Some(UNSUPPORTED_CONFIG_SCHEMA_CODE));

        Ok(())
    }

    #[test]
    fn pre_existing_configuration_failures_keep_their_untyped_envelope() -> Result<()> {
        // These three shapes predate the schema registry. They stay generic
        // command failures so the machine-readable envelope consumers already
        // parse does not change code or lose `details.causes`.
        let project = tempdir()?;
        write_project_config(project.path(), "hybrid_retrieval_enabled = \n")?;
        let parse_failure = load_reporting_to_user(project.path(), &isolated_startup())
            .expect_err("a malformed file fails");
        assert_eq!(failure_code(&parse_failure), None);
        assert!(format!("{parse_failure:#}").contains("Failed to parse config"));

        write_project_config(project.path(), "cache_dir = \"/repo-controlled\"\n")?;
        let untrusted = load_reporting_to_user(project.path(), &isolated_startup())
            .expect_err("a project cache_dir fails");
        assert_eq!(failure_code(&untrusted), None);
        assert!(format!("{untrusted:#}").contains("is not trusted"));

        Ok(())
    }

    /// A registered key that no field reads is a documented knob that does
    /// nothing, and a field with no registered key is an undocumented one.
    /// Serializing the loaded config is what ties the registry to the struct;
    /// asserting that registered keys are not "unknown" only restates the
    /// registry to itself.
    #[test]
    fn every_registered_config_key_is_read_into_a_field() -> Result<()> {
        // The trusted user home file is the only source allowed to carry every
        // registered key, so it is where completeness can be proven.
        let home = tempdir()?;
        let project = tempdir()?;
        let body = config_registry::CONFIG_FILE_KEYS
            .iter()
            .map(|entry| match entry.kind {
                config_registry::SettingKind::Boolean => format!("{} = true", entry.key),
                config_registry::SettingKind::Integer => format!("{} = 1", entry.key),
                _ => format!("{} = \"value\"", entry.key),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(home.path().join(".codestory.toml"), body)?;

        let mut startup = isolated_startup();
        startup.user_home = Some(home.path().to_path_buf());
        let (config, reported) = load_reporting_to_user(project.path(), &startup)?;
        assert!(
            reported.is_empty(),
            "registered keys must not be reported as unknown: {reported:?}"
        );

        let loaded = serde_json::to_value(&config)?;
        let fields = loaded
            .as_object()
            .expect("the configuration serializes as a map");
        for entry in config_registry::CONFIG_FILE_KEYS {
            // The schema version steers the loader itself; it is not a runtime
            // setting, so no `CliConfig` field carries it.
            if entry.key == CONFIG_SCHEMA_VERSION_KEY {
                assert!(
                    !fields.contains_key(entry.key),
                    "{} is the loader's own key and must not become a setting field",
                    entry.key
                );
                continue;
            }
            let value = fields.get(entry.key).unwrap_or_else(|| {
                panic!(
                    "registered key `{}` has no CliConfig field; the documented key does nothing",
                    entry.key
                )
            });
            assert!(
                !value.is_null(),
                "registered key `{}` exists as a field but the loader did not read it",
                entry.key
            );
        }
        for field in fields.keys() {
            assert!(
                config_registry::is_registered_config_key(field),
                "CliConfig field `{field}` is honoured but not registered, so it is undocumented \
                 and schema version 2 would reject it"
            );
        }

        Ok(())
    }
}
