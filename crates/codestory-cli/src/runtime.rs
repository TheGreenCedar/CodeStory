//! Runtime boundary between CLI commands and CodeStory services.
//!
//! Command handlers use this module to resolve a project, choose the cache root,
//! open the runtime services, refresh indexes when requested, and translate API
//! failures into CLI-facing errors. Keep command-specific output formatting out
//! of this layer.

use anyhow::{Context, Result, anyhow, bail};
use codestory_contracts::api::{
    ApiError, AppEventPayload, IndexMode, IndexingPhaseTimings, ProjectSummary, SearchHit,
};
use codestory_runtime::{
    BookmarkService, GroundingService, IndexService, ProjectService, ReadOnlyBrowserService,
    Runtime, TargetResolution,
};
use std::path::{Path, PathBuf};

use crate::args::{ProjectArgs, QuerySelectorOutput, RefreshMode, TargetSelection};
use crate::display::{clean_path_string, quote_command_path};

const INCOMPLETE_INDEX_RECOVERY_REASON: &str =
    "previous_incremental_run_incomplete_full_refresh_required";

#[derive(Debug)]
/// Project state after a command has opened or refreshed the repository.
///
/// `refresh_mode` records the resolved indexing action, not merely the user
/// request, so output can distinguish `auto(full)` from `auto(incremental)`.
pub(crate) struct OpenedProject {
    pub(crate) summary: ProjectSummary,
    pub(crate) refresh_mode: Option<IndexMode>,
    pub(crate) phase_timings: Option<IndexingPhaseTimings>,
}

/// Shared service handles and filesystem roots for a CLI command.
///
/// Runtime construction never promotes installed managed embedding assets into
/// product defaults. Product packet/search paths set llama.cpp sidecar defaults
/// explicitly before opening the runtime.
pub(crate) struct RuntimeContext {
    pub(crate) project: ProjectService,
    pub(crate) index: IndexService,
    pub(crate) grounding: GroundingService,
    pub(crate) bookmarks: BookmarkService,
    pub(crate) browser: ReadOnlyBrowserService,
    pub(crate) events: crossbeam_channel::Receiver<AppEventPayload>,
    pub(crate) project_root: PathBuf,
    pub(crate) cache_root: PathBuf,
    pub(crate) storage_path: PathBuf,
    pub(crate) sidecar: codestory_retrieval::SidecarRuntimeConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTarget {
    pub(crate) selector: QuerySelectorOutput,
    pub(crate) requested: String,
    pub(crate) file_filter: Option<String>,
    pub(crate) selected: SearchHit,
    pub(crate) alternatives: Vec<SearchHit>,
}

impl ResolvedTarget {
    pub(crate) fn from_runtime(target: codestory_runtime::ResolvedTarget) -> Self {
        Self {
            selector: match target.selector {
                codestory_runtime::TargetSelector::Id => QuerySelectorOutput::Id,
                codestory_runtime::TargetSelector::Query => QuerySelectorOutput::Query,
            },
            requested: target.requested,
            file_filter: target.file_filter,
            selected: target.selected,
            alternatives: target.alternatives,
        }
    }
}

#[derive(Debug, Clone)]
/// Error returned when a query has multiple equally valid CLI targets.
///
/// Handlers render this as structured resolution output where possible so
/// callers can rerun with `--choose` instead of guessing.
pub(crate) struct AmbiguousTargetError {
    pub(crate) query: String,
    pub(crate) file_filter: Option<String>,
    pub(crate) alternatives: Vec<SearchHit>,
    pub(crate) message: String,
}

impl std::fmt::Display for AmbiguousTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AmbiguousTargetError {}

impl RuntimeContext {
    /// Open runtime services with the caller's embedding configuration.
    pub(crate) fn new(args: &ProjectArgs) -> Result<Self> {
        Self::new_with_startup(args, &crate::config::process_startup_config())
    }

    /// Open runtime services for agent-facing packet/search commands.
    #[cfg(test)]
    pub(crate) fn new_agent_sidecar(args: &ProjectArgs) -> Result<Self> {
        Self::new_agent_sidecar_with_selection(args, None, None)
    }

    pub(crate) fn new_agent_sidecar_with_startup(
        args: &ProjectArgs,
        startup: &crate::config::CliStartupConfig,
    ) -> Result<Self> {
        Self::new_agent_sidecar_with_startup_and_selection(args, startup, None, None)
    }

    pub(crate) fn new_agent_sidecar_with_selection(
        args: &ProjectArgs,
        profile: Option<crate::args::CliSidecarProfile>,
        run_id: Option<&str>,
    ) -> Result<Self> {
        Self::new_agent_sidecar_with_startup_and_selection(
            args,
            &crate::config::process_startup_config(),
            profile,
            run_id,
        )
    }

    fn new_agent_sidecar_with_startup_and_selection(
        args: &ProjectArgs,
        startup: &crate::config::CliStartupConfig,
        profile: Option<crate::args::CliSidecarProfile>,
        run_id: Option<&str>,
    ) -> Result<Self> {
        let mut context = Self::new_with_startup(args, startup)?;
        let selected = profile
            .map(Into::into)
            .unwrap_or(codestory_retrieval::SidecarProfile::Agent);
        context.sidecar =
            context
                .sidecar
                .with_profile_and_run_id(Some(&context.project_root), selected, run_id);
        let runtime = Runtime::new_with_config(context.sidecar.clone());
        context.project = runtime.project_service();
        context.index = runtime.index_service();
        context.grounding = runtime.grounding_service();
        context.bookmarks = runtime.bookmark_service();
        context.browser = runtime.browser_service();
        context.events = runtime.events();
        Ok(context)
    }

    /// Open runtime services without starting managed embedding processes.
    pub(crate) fn new_inspect_only(args: &ProjectArgs) -> Result<Self> {
        Self::new(args)
    }

    fn new_with_startup(
        args: &ProjectArgs,
        startup: &crate::config::CliStartupConfig,
    ) -> Result<Self> {
        let project_root = canonicalize_project_root(&args.project)?;
        let config = crate::config::load_config_with_startup(&project_root, startup)?;
        let cache_override = args.cache_dir.clone().or_else(|| config.cache_dir.clone());
        let process_cache_root = startup
            .stdio_cache_root
            .as_deref()
            .unwrap_or_else(|| startup.sidecar_defaults.cache_root());
        let cache_root = cache_root_for_project_in(
            &project_root,
            cache_override.as_deref(),
            process_cache_root,
        )?;
        let storage_path = cache_root.join("codestory.db");
        let sidecar_defaults = startup
            .sidecar_defaults
            .with_cache_root(process_cache_root.to_path_buf());
        let sidecar = crate::sidecar_runtime::for_project_auto_with_process_defaults(
            &project_root,
            &sidecar_defaults,
            &config.runtime_overrides(),
        );
        let runtime = Runtime::new_with_config(sidecar.clone());
        let events = runtime.events();
        Ok(Self {
            project: runtime.project_service(),
            index: runtime.index_service(),
            grounding: runtime.grounding_service(),
            bookmarks: runtime.bookmark_service(),
            browser: runtime.browser_service(),
            events,
            project_root,
            cache_root,
            storage_path,
            sidecar,
        })
    }

    /// Open the project and run the resolved refresh request when needed.
    ///
    /// `RefreshMode::None` is read-only with respect to indexing; commands that
    /// require cached graph data must call `ensure_index_ready` after this.
    pub(crate) fn ensure_open(&self, refresh: RefreshMode) -> Result<OpenedProject> {
        let summary = self.open_project_summary()?;
        self.ensure_open_from_summary(refresh, summary)
    }

    /// Open project state from an already-read summary.
    ///
    /// This keeps commands such as drill from reading the same summary twice
    /// before deciding whether a refresh is necessary.
    pub(crate) fn ensure_open_from_summary(
        &self,
        refresh: RefreshMode,
        mut summary: ProjectSummary,
    ) -> Result<OpenedProject> {
        let refresh_mode = resolve_refresh_request(refresh, &summary);
        let mut phase_timings = None;
        if let Some(mode) = refresh_mode {
            phase_timings = Some(
                self.index
                    .run_indexing_blocking_without_runtime_refresh(mode)
                    .map_err(map_api_error)?,
            );
            summary = self.open_project_summary()?;
        }

        Ok(OpenedProject {
            summary,
            refresh_mode,
            phase_timings,
        })
    }

    pub(crate) fn ensure_ground_open(&self, refresh: RefreshMode) -> Result<OpenedProject> {
        self.ensure_open(refresh)
    }

    /// Open the project summary using the resolved storage path.
    pub(crate) fn open_project_summary(&self) -> Result<ProjectSummary> {
        self.project
            .open_project_summary_with_storage_path(
                self.project_root.clone(),
                self.storage_path.clone(),
            )
            .map_err(|error| map_api_error_for_project(error, &self.project_root))
    }
}

/// Resolve a CLI target selector into one graph-backed search hit.
///
/// Id selectors bypass search. Query selectors use indexed symbol candidates,
/// apply the optional file filter, and fail on ambiguous top-ranked matches so
/// command handlers do not silently pick the wrong symbol.
pub(crate) fn resolve_target(
    runtime: &RuntimeContext,
    target: TargetSelection,
    file_filter: Option<&str>,
) -> Result<ResolvedTarget> {
    let target = match target {
        TargetSelection::Id(id) => codestory_runtime::TargetSelection::Id(id),
        TargetSelection::Query { query, choose } => {
            codestory_runtime::TargetSelection::Query { query, choose }
        }
    };
    match runtime.browser.resolve_target(target, file_filter) {
        Ok(TargetResolution::Resolved(target)) => Ok(ResolvedTarget::from_runtime(*target)),
        Ok(TargetResolution::Ambiguous(ambiguous)) => Err(AmbiguousTargetError {
            query: ambiguous.query,
            file_filter: ambiguous.file_filter,
            alternatives: ambiguous.alternatives,
            message: ambiguous.message,
        }
        .into()),
        Ok(TargetResolution::Rejected(message)) => Err(anyhow!(message)),
        Err(error) => Err(map_api_error(error)),
    }
}

/// Canonicalize a project argument before deriving cache identity.
pub(crate) fn canonicalize_project_root(project: &Path) -> Result<PathBuf> {
    let project = if project.is_absolute() {
        project.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current working directory")?
            .join(project)
    };

    project.canonicalize().with_context(|| {
        format!(
            "Failed to resolve project path `{}`. Ensure the directory exists.",
            clean_path_string(&project.to_string_lossy())
        )
    })
}

/// Return the cache directory used for one project.
///
/// Explicit overrides are returned unchanged; otherwise the cache root is a
/// stable hash of the canonical project path under the platform cache directory.
#[cfg(test)]
pub(crate) fn cache_root_for_project(
    project_root: &Path,
    override_dir: Option<&Path>,
) -> Result<PathBuf> {
    cache_root_for_project_in(
        project_root,
        override_dir,
        &crate::sidecar_runtime::user_cache_root(),
    )
}

fn cache_root_for_project_in(
    project_root: &Path,
    override_dir: Option<&Path>,
    process_cache_root: &Path,
) -> Result<PathBuf> {
    match override_dir {
        Some(path) => Ok(path.to_path_buf()),
        None => Ok(process_cache_root.join(fnv1a_hex(project_root.to_string_lossy().as_bytes()))),
    }
}

/// Small stable hash used for path-derived cache directory names.
pub(crate) fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Convert a requested refresh mode into the indexing action to run.
pub(crate) fn resolve_refresh_request(
    refresh: RefreshMode,
    summary: &ProjectSummary,
) -> Option<IndexMode> {
    let requires_full_recovery = summary
        .freshness
        .as_ref()
        .and_then(|freshness| freshness.reason.as_deref())
        == Some(INCOMPLETE_INDEX_RECOVERY_REASON);
    match refresh {
        RefreshMode::Auto => Some(if summary.stats.node_count == 0 || requires_full_recovery {
            IndexMode::Full
        } else {
            IndexMode::Incremental
        }),
        RefreshMode::Full => Some(IndexMode::Full),
        RefreshMode::Incremental => Some(if requires_full_recovery {
            IndexMode::Full
        } else {
            IndexMode::Incremental
        }),
        RefreshMode::None => None,
    }
}

/// Human-readable label for the requested and resolved refresh pair.
pub(crate) fn refresh_label(requested: RefreshMode, resolved: Option<IndexMode>) -> String {
    match (requested, resolved) {
        (RefreshMode::Auto, Some(IndexMode::Full)) => "auto(full)".to_string(),
        (RefreshMode::Auto, Some(IndexMode::Incremental)) => "auto(incremental)".to_string(),
        (RefreshMode::Full, Some(_)) => "full".to_string(),
        (RefreshMode::Incremental, Some(IndexMode::Full)) => {
            "incremental(recovery-full)".to_string()
        }
        (RefreshMode::Incremental, Some(IndexMode::Incremental)) => "incremental".to_string(),
        (RefreshMode::None, None) => "none".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Fail read commands early when the cache has no indexed graph data.
pub(crate) fn ensure_index_ready(opened: &OpenedProject, subcommand: &str) -> Result<()> {
    if opened.summary.stats.node_count == 0 {
        let project = clean_path_string(&opened.summary.root);
        bail!(
            "No indexed files are available for `{subcommand}` in `{project}`.\n\n`{subcommand}` only reads the existing cache unless you pass `--refresh`.\nRun `codestory-cli index --project \"{project}\" --refresh full` to build the cache first.\nIf you want the read command to refresh on demand, rerun it with `--refresh incremental` or `--refresh full`."
        );
    }
    Ok(())
}

/// Map runtime API errors into CLI errors with repair commands when available.
pub(crate) fn map_api_error(error: ApiError) -> anyhow::Error {
    map_api_error_with_project(error, None)
}

/// Map runtime API errors with project-specific cache repair guidance.
pub(crate) fn map_api_error_for_project(error: ApiError, project: &Path) -> anyhow::Error {
    map_api_error_with_project(error, Some(project))
}

fn map_api_error_with_project(error: ApiError, project: Option<&Path>) -> anyhow::Error {
    if api_error_is_cache_busy(&error) {
        return anyhow!(cache_busy_message(project));
    }
    let mut message = format!("{}: {}", error.code, error.message);
    if let Some((minimum_next, full_repair)) = api_error_repair_groups(&error) {
        if !minimum_next.is_empty() {
            message.push_str("\n\nMinimum next:");
            for command in minimum_next {
                message.push_str("\n  ");
                message.push_str(command);
            }
        }
        if !full_repair.is_empty() && full_repair != minimum_next {
            message.push_str("\n\nFull repair:");
            for command in full_repair {
                message.push_str("\n  ");
                message.push_str(command);
            }
        }
    } else if let Some(next_commands) = api_error_next_commands(&error) {
        message.push_str("\n\nNext commands:");
        for command in next_commands {
            message.push_str("\n  ");
            message.push_str(&command);
        }
    }
    anyhow!(message)
}

/// Rewrite cache-busy errors from non-API paths into the standard CLI message.
pub(crate) fn map_cache_busy_anyhow(error: anyhow::Error, project: &Path) -> anyhow::Error {
    if is_cache_busy_text(&error.to_string()) {
        return anyhow!(cache_busy_message(Some(project)));
    }
    error
}

fn api_error_repair_groups(error: &ApiError) -> Option<(&[String], &[String])> {
    let details = error.details.as_ref()?;
    if details.minimum_next.is_empty() && details.full_repair.is_empty() {
        return details.readiness.as_ref().map(|verdict| {
            (
                verdict.minimum_next.as_slice(),
                verdict.full_repair.as_slice(),
            )
        });
    }
    Some((&details.minimum_next, &details.full_repair))
}

fn api_error_next_commands(error: &ApiError) -> Option<Vec<String>> {
    let commands = &error.details.as_ref()?.next_commands;
    (!commands.is_empty()).then_some(commands.clone())
}

fn api_error_is_cache_busy(error: &ApiError) -> bool {
    let text = format!("{} {}", error.code, error.message).to_ascii_lowercase();
    is_cache_busy_text(&text)
}

fn is_cache_busy_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("database is locked") || text.contains("sqlite_busy")
}

fn cache_busy_message(project: Option<&Path>) -> String {
    let project = project
        .map(quote_command_path)
        .unwrap_or_else(|| "<repo>".to_string());
    format!(
        "cache_busy: CodeStory cache is busy or locked. Wait for the active indexing/search process to release the SQLite cache, then retry.\n\nMinimum next:\n  codestory-cli ready --project {project} --goal agent\n\nFull repair:\n  codestory-cli ready --project {project} --goal agent\n  codestory-cli doctor --project {project}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use tempfile::tempdir;

    const MANAGED_ENV_VARS: &[&str] = &[
        "CODESTORY_EMBED_RUNTIME_MODE",
        "CODESTORY_EMBED_BACKEND",
        "CODESTORY_EMBED_PORT",
        "CODESTORY_EMBED_LLAMACPP_URL",
        "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE",
        "CODESTORY_SEMANTIC_DOC_MAX_TOKENS",
        "CODESTORY_STORED_VECTOR_ENCODING",
        "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
    ];
    const HOME_ENV_VARS: &[&str] = &["USERPROFILE", "HOME"];

    struct EnvSnapshot {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvSnapshot {
        fn clear(names: &'static [&'static str]) -> Self {
            let values = names
                .iter()
                .map(|name| (*name, env::var_os(name)))
                .collect::<Vec<_>>();
            unsafe {
                for name in names {
                    env::remove_var(name);
                }
            }
            Self { values }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            unsafe {
                for (name, value) in &self.values {
                    if let Some(value) = value {
                        env::set_var(name, value);
                    } else {
                        env::remove_var(name);
                    }
                }
            }
        }
    }

    #[test]
    fn project_config_cache_dir_is_rejected_before_runtime_paths() {
        let _env_lock = crate::config::config_env_test_lock();
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("repo");
        let config_cache = temp.path().join("repo-controlled-cache");
        fs::create_dir_all(&project).expect("create project");
        fs::write(
            project.join(".codestory.toml"),
            format!("cache_dir = {:?}\n", config_cache.to_string_lossy()),
        )
        .expect("write project config");

        let err = match RuntimeContext::new_inspect_only(&ProjectArgs {
            project,
            cache_dir: None,
        }) {
            Ok(_) => panic!("repo-controlled cache_dir should fail closed"),
            Err(err) => err,
        };

        let message = format!("{err:#}");
        assert!(message.contains("project config field `cache_dir` is not trusted"));
        assert!(message.contains("--cache-dir"));
    }

    #[test]
    fn home_config_cache_dir_resolves_storage_under_trusted_root() {
        let _env_lock = crate::config::config_env_test_lock();
        let _managed_env = EnvSnapshot::clear(MANAGED_ENV_VARS);
        let _home_env = EnvSnapshot::clear(HOME_ENV_VARS);
        let temp = tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let project = temp.path().join("repo");
        let cache = temp.path().join("trusted-cache");
        fs::create_dir_all(&home).expect("create home");
        fs::create_dir_all(&project).expect("create project");
        fs::write(
            home.join(".codestory.toml"),
            format!("cache_dir = {:?}\n", cache.to_string_lossy()),
        )
        .expect("write home config");
        unsafe {
            env::set_var("USERPROFILE", &home);
        }

        let runtime = RuntimeContext::new(&ProjectArgs {
            project,
            cache_dir: None,
        })
        .expect("runtime context");

        assert_eq!(runtime.cache_root, cache);
        assert_eq!(runtime.storage_path, cache.join("codestory.db"));
    }

    #[test]
    fn cli_cache_dir_overrides_home_config_for_storage() {
        let _env_lock = crate::config::config_env_test_lock();
        let _managed_env = EnvSnapshot::clear(MANAGED_ENV_VARS);
        let _home_env = EnvSnapshot::clear(HOME_ENV_VARS);
        let temp = tempdir().expect("temp dir");
        let home = temp.path().join("home");
        let project = temp.path().join("repo");
        let home_cache = temp.path().join("home-cache");
        let cli_cache = temp.path().join("cli-cache");
        fs::create_dir_all(&home).expect("create home");
        fs::create_dir_all(&project).expect("create project");
        fs::write(
            home.join(".codestory.toml"),
            format!("cache_dir = {:?}\n", home_cache.to_string_lossy()),
        )
        .expect("write home config");
        unsafe {
            env::set_var("USERPROFILE", &home);
        }

        let runtime = RuntimeContext::new(&ProjectArgs {
            project,
            cache_dir: Some(cli_cache.clone()),
        })
        .expect("runtime context");

        assert_eq!(runtime.cache_root, cli_cache);
        assert_eq!(runtime.storage_path, cli_cache.join("codestory.db"));
    }

    #[test]
    fn explicit_startup_snapshots_isolate_concurrent_runtime_paths_and_endpoints() {
        let temp = tempdir().expect("temp dir");
        let first_project = temp.path().join("first-project");
        let second_project = temp.path().join("second-project");
        let first_cache = temp.path().join("first-cache");
        let second_cache = temp.path().join("second-cache");
        fs::create_dir_all(&first_project).expect("create first project");
        fs::create_dir_all(&second_project).expect("create second project");
        fs::write(
            first_project.join(".codestory.toml"),
            r#"embedding_endpoint = "http://127.0.0.1:41001/v1/embeddings""#,
        )
        .expect("write first config");
        fs::write(
            second_project.join(".codestory.toml"),
            r#"embedding_endpoint = "http://127.0.0.1:41002/v1/embeddings""#,
        )
        .expect("write second config");
        let startup = |cache_root: &Path| crate::config::CliStartupConfig {
            user_home: None,
            project_network_config_allowed: true,
            stdio_cache_root: Some(cache_root.to_path_buf()),
            sidecar_defaults: codestory_retrieval::SidecarProcessDefaults::new(
                cache_root.to_path_buf(),
                codestory_retrieval::SidecarRuntimeDefaults::default(),
            ),
        };
        let first_startup = startup(&first_cache);
        let second_startup = startup(&second_cache);

        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                RuntimeContext::new_agent_sidecar_with_startup(
                    &ProjectArgs {
                        project: first_project.clone(),
                        cache_dir: None,
                    },
                    &first_startup,
                )
                .expect("first runtime")
            });
            let second = scope.spawn(|| {
                RuntimeContext::new_agent_sidecar_with_startup(
                    &ProjectArgs {
                        project: second_project.clone(),
                        cache_dir: None,
                    },
                    &second_startup,
                )
                .expect("second runtime")
            });
            (
                first.join().expect("first runtime worker"),
                second.join().expect("second runtime worker"),
            )
        });

        assert!(first.storage_path.starts_with(&first_cache));
        assert!(second.storage_path.starts_with(&second_cache));
        assert!(first.sidecar.layout.state_file.starts_with(&first_cache));
        assert!(second.sidecar.layout.state_file.starts_with(&second_cache));
        assert_eq!(
            first.sidecar.embedding.endpoint,
            "http://127.0.0.1:41001/v1/embeddings"
        );
        assert_eq!(
            second.sidecar.embedding.endpoint,
            "http://127.0.0.1:41002/v1/embeddings"
        );
    }

    #[test]
    fn agent_sidecar_runtime_defaults_to_bundled_llamacpp() {
        let _env_lock = crate::config::config_env_test_lock();
        let _env_snapshot = EnvSnapshot::clear(MANAGED_ENV_VARS);
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("repo");
        let cache = temp.path().join("cache");
        fs::create_dir_all(&project).expect("create project");

        let runtime = RuntimeContext::new_agent_sidecar(&ProjectArgs {
            project: project.clone(),
            cache_dir: Some(cache),
        })
        .expect("runtime context");

        assert_eq!(
            runtime.sidecar.profile,
            codestory_retrieval::SidecarProfile::Agent
        );
        assert_eq!(runtime.sidecar.embedding.backend, "llamacpp");
        assert_eq!(env::var("CODESTORY_EMBED_LLAMACPP_URL").ok(), None);
        let sidecar = runtime.sidecar.with_profile_and_run_id(
            Some(&project),
            codestory_retrieval::SidecarProfile::Agent,
            Some("ready-repair-test"),
        );
        let expected_url =
            codestory_retrieval::SidecarLayout::embed_base_url(sidecar.embed_http_port);
        assert_eq!(sidecar.embedding.endpoint.as_str(), expected_url.as_str());
        assert_ne!(expected_url, "http://127.0.0.1:8080/v1/embeddings");
    }

    #[test]
    fn bundled_llamacpp_defaults_preserve_explicit_user_env() {
        let _env_lock = crate::config::config_env_test_lock();
        let _env_snapshot = EnvSnapshot::clear(MANAGED_ENV_VARS);
        unsafe {
            env::set_var("CODESTORY_EMBED_BACKEND", "llamacpp");
            env::set_var(
                "CODESTORY_EMBED_LLAMACPP_URL",
                "http://127.0.0.1:18080/v1/embeddings",
            );
        }

        let project = tempfile::tempdir().expect("project");
        let runtime = RuntimeContext::new_agent_sidecar(&ProjectArgs {
            project: project.path().to_path_buf(),
            cache_dir: None,
        })
        .expect("runtime context");

        assert_eq!(
            env::var("CODESTORY_EMBED_BACKEND").ok().as_deref(),
            Some("llamacpp")
        );
        assert_eq!(
            runtime.sidecar.embedding.endpoint.as_str(),
            "http://127.0.0.1:18080/v1/embeddings"
        );
    }
}
