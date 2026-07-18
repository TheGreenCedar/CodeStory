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
    ActivationService, BookmarkService, GroundingService, IndexService, ProjectService,
    PublicOperation, PublicOperationService, ReadOnlyBrowserService, Runtime, TargetResolution,
};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
/// The retrieval runtime carries immutable process defaults and project
/// artifact paths. Product surfaces initialize the shared embedded engine when
/// they need semantic work; inspect-only surfaces remain observational.
pub(crate) struct RuntimeContext {
    pub(crate) activation: ActivationService,
    pub(crate) public_operation: PublicOperationService,
    pub(crate) project: ProjectService,
    pub(crate) index: IndexService,
    pub(crate) grounding: GroundingService,
    pub(crate) bookmarks: BookmarkService,
    pub(crate) browser: ReadOnlyBrowserService,
    pub(crate) events: crossbeam_channel::Receiver<AppEventPayload>,
    pub(crate) project_root: PathBuf,
    /// Stable logical/workspace identity plus the immutable runtime
    /// configuration used to build this context. Multi-project transports use
    /// this key instead of a path spelling, mutable artifact eligibility, or a
    /// later process-environment read.
    pub(crate) context_key: ProjectContextKey,
    pub(crate) cache_root: PathBuf,
    pub(crate) storage_path: PathBuf,
    pub(crate) sidecar: codestory_retrieval::SidecarRuntimeConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProjectContextKey {
    pub(crate) project_id: String,
    pub(crate) workspace_id: String,
    pub(crate) configuration_id: String,
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
        context.context_key.configuration_id =
            runtime_configuration_id(&context.cache_root, &context.sidecar);
        let runtime = Runtime::new_with_config(context.sidecar.clone());
        context.project = runtime.project_service();
        context.index = runtime.index_service();
        context.grounding = runtime.grounding_service();
        context.bookmarks = runtime.bookmark_service();
        context.browser = runtime.browser_service();
        context.activation = runtime.activation_service();
        context.public_operation = runtime.public_operation_service();
        context.events = runtime.events();
        Ok(context)
    }

    /// Open runtime services without initializing the embedded engine.
    pub(crate) fn new_inspect_only(args: &ProjectArgs) -> Result<Self> {
        Self::new(args)
    }

    #[cfg(test)]
    pub(crate) fn new_inspect_only_with_startup(
        args: &ProjectArgs,
        startup: &crate::config::CliStartupConfig,
    ) -> Result<Self> {
        Self::new_with_startup(args, startup)
    }

    fn new_with_startup(
        args: &ProjectArgs,
        startup: &crate::config::CliStartupConfig,
    ) -> Result<Self> {
        let project_root = canonicalize_project_root(&args.project)?;
        let project_identity = codestory_workspace::project_identity_v3(&project_root);
        let config = crate::config::load_config_with_startup(&project_root, startup)?;
        let cache_override = args.cache_dir.clone().or_else(|| config.cache_dir.clone());
        let process_cache_root = canonicalize_configuration_path(
            startup
                .stdio_cache_root
                .as_deref()
                .unwrap_or_else(|| startup.sidecar_defaults.cache_root()),
        )?;
        let cache_root = cache_root_for_project_in(
            &project_root,
            cache_override.as_deref(),
            &process_cache_root,
        )?;
        let storage_path = canonicalize_configuration_path(&cache_root.join("codestory.db"))?;
        let sidecar_defaults = startup.sidecar_defaults.with_cache_root(process_cache_root);
        let sidecar = crate::sidecar_runtime::for_project_auto_with_process_defaults(
            &project_root,
            &sidecar_defaults,
            &config.runtime_overrides(),
        );
        let context_key = ProjectContextKey {
            project_id: project_identity.project_id.clone(),
            workspace_id: project_identity.workspace_id.clone(),
            configuration_id: runtime_configuration_id(&cache_root, &sidecar),
        };
        let runtime = Runtime::new_with_config(sidecar.clone());
        let events = runtime.events();
        Ok(Self {
            activation: runtime.activation_service(),
            public_operation: runtime.public_operation_service(),
            project: runtime.project_service(),
            index: runtime.index_service(),
            grounding: runtime.grounding_service(),
            bookmarks: runtime.bookmark_service(),
            browser: runtime.browser_service(),
            events,
            project_root,
            context_key,
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

    pub(crate) fn inspect_project_summary(&self) -> Result<Option<ProjectSummary>> {
        self.project
            .inspect_project_summary_with_storage_path(
                self.project_root.clone(),
                self.storage_path.clone(),
            )
            .map_err(|error| map_api_error_for_project(error, &self.project_root))
    }

    /// Keep all reads that contribute to one CLI response under the runtime's
    /// complete core/retrieval publication pin. Output writes stay outside the
    /// closure so the runtime's single bounded retry cannot duplicate them.
    pub(crate) fn run_public_operation<T>(
        &self,
        operation: &str,
        mut build: impl FnMut() -> Result<T>,
    ) -> Result<PublicOperation<T>> {
        let mut build_error = None;
        let result = self.public_operation.run_with_cancel(
            operation,
            Arc::new(AtomicBool::new(false)),
            || match build() {
                Ok(value) => Ok(value),
                Err(error) => {
                    let message = error.to_string();
                    build_error = Some(error);
                    Err(ApiError::new("cli_public_operation_failed", message))
                }
            },
        );
        match result {
            Ok(operation) => Ok(operation),
            Err(error) if error.code == "cli_public_operation_failed" => {
                Err(build_error.expect("CLI operation error was retained"))
            }
            Err(error) => Err(map_api_error_for_project(error, &self.project_root)),
        }
    }

    /// Pin one complete publication without requiring current source
    /// freshness. This is reserved for surfaces such as `affected` whose job
    /// is to explain drift from that publication.
    pub(crate) fn run_observational_public_operation<T>(
        &self,
        operation: &str,
        mut build: impl FnMut() -> Result<T>,
    ) -> Result<PublicOperation<T>> {
        let mut build_error = None;
        let result = self.public_operation.run_observational_with_cancel(
            operation,
            Arc::new(AtomicBool::new(false)),
            || match build() {
                Ok(value) => Ok(value),
                Err(error) => {
                    let message = error.to_string();
                    build_error = Some(error);
                    Err(ApiError::new("cli_public_operation_failed", message))
                }
            },
        );
        match result {
            Ok(operation) => Ok(operation),
            Err(error) if error.code == "cli_public_operation_failed" => {
                Err(build_error.expect("CLI operation error was retained"))
            }
            Err(error) => Err(map_api_error_for_project(error, &self.project_root)),
        }
    }

    pub(crate) fn active_project_summary(&self) -> Result<ProjectSummary> {
        self.public_operation
            .active_project_summary()
            .map_err(|error| map_api_error_for_project(error, &self.project_root))
    }
}

/// Serialize one publication-backed response with the canonical adapter
/// metadata envelope. HTTP and ordinary CLI JSON use this exact helper so the
/// shape cannot drift between transports.
pub(crate) fn public_operation_json_value<T, V: Serialize>(
    operation: &PublicOperation<T>,
    response: &V,
) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(response)?;
    if !value.is_object() {
        value = serde_json::json!({"result": value});
    }
    let core_publication = &operation.core_publication;
    let retrieval_publication = &operation.retrieval_publication;
    let object = value
        .as_object_mut()
        .expect("public operation payload is an object");
    let metadata = object
        .entry("_meta")
        .or_insert_with(|| serde_json::json!({}));
    if !metadata.is_object() {
        *metadata = serde_json::json!({});
    }
    metadata
        .as_object_mut()
        .expect("public operation metadata is an object")
        .insert(
            "codestory_publication".to_string(),
            serde_json::json!({
                "served_from": "complete_publication",
                "publication": core_publication,
                "core_publication": core_publication,
                "retrieval_publication": retrieval_publication,
                "operation": {
                    "operation_id": &operation.operation_id,
                    "attempt": operation.attempt,
                }
            }),
        );
    Ok(value)
}

pub(crate) fn map_public_operation<T, U>(
    operation: PublicOperation<T>,
    map: impl FnOnce(T) -> U,
) -> PublicOperation<U> {
    PublicOperation {
        value: map(operation.value),
        core_publication: operation.core_publication,
        retrieval_publication: operation.retrieval_publication,
        operation_id: operation.operation_id,
        attempt: operation.attempt,
    }
}

fn runtime_configuration_id(
    cache_root: &Path,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> String {
    // Do not hash credentials. Their presence participates in the immutable
    // configuration boundary, while secret material remains outside logs and
    // diagnostic identifiers.
    let mut identity = format!(
        "{}\0{:?}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        configuration_path_identity(cache_root),
        sidecar.profile,
        sidecar.namespace,
        sidecar.embedding.allow_cpu,
        sidecar.retrieval.hybrid_enabled,
        sidecar.retrieval.semantic_doc_scope,
        sidecar.retrieval.semantic_doc_alias_mode,
        sidecar.retrieval.semantic_doc_max_tokens,
        sidecar.retrieval.llm_doc_embed_batch_size,
        sidecar.retrieval.stream_pending_docs,
        sidecar.retrieval.stream_sort_window_batches,
        sidecar.summary.endpoint.as_deref().unwrap_or(""),
        sidecar.summary.api_key.is_some(),
    );
    identity.push_str(&format!(
        "\0{}\0{:?}\0{:?}\0{}\0{}\0{}\0{}\0{}",
        sidecar.summary.model,
        sidecar.summary.max_tokens,
        sidecar.summary.timeout,
        sidecar.run_id.as_deref().unwrap_or(""),
        configuration_path_identity(&sidecar.layout.lexical_data_dir),
        configuration_path_identity(&sidecar.layout.semantic_data_dir),
        configuration_path_identity(&sidecar.layout.scip_artifacts_root),
        configuration_path_identity(&sidecar.layout.state_file),
    ));
    fnv1a_hex(identity.as_bytes())
}

fn configuration_path_identity(path: &Path) -> String {
    #[cfg(windows)]
    {
        windows_ordinal_configuration_path_identity(path)
    }
    #[cfg(not(windows))]
    {
        clean_path_string(&path.to_string_lossy())
    }
}

#[cfg(windows)]
fn windows_ordinal_configuration_path_identity(path: &Path) -> String {
    use std::ptr;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LCMapStringEx(
            locale_name: *const u16,
            map_flags: u32,
            source: *const u16,
            source_len: i32,
            destination: *mut u16,
            destination_len: i32,
            version_information: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
            sort_handle: isize,
        ) -> i32;
    }

    const LCMAP_UPPERCASE: u32 = 0x0000_0200;
    let normalized = clean_path_string(&path.to_string_lossy()).replace('/', "\\");
    let source = normalized.encode_utf16().collect::<Vec<_>>();
    let Ok(source_len) = i32::try_from(source.len()) else {
        return normalized.to_uppercase();
    };
    let invariant_locale = [0_u16];
    // SAFETY: all pointers remain valid for the supplied lengths. The invariant
    // locale uses the same language-independent uppercase table as Windows
    // ordinal ignore-case comparison.
    let required = unsafe {
        LCMapStringEx(
            invariant_locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            source_len,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if required <= 0 {
        return normalized.to_uppercase();
    }
    let mut mapped = vec![0_u16; required as usize];
    // SAFETY: `mapped` has the size returned by the preceding mapping query.
    let written = unsafe {
        LCMapStringEx(
            invariant_locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            source_len,
            mapped.as_mut_ptr(),
            required,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if written <= 0 {
        return normalized.to_uppercase();
    }
    String::from_utf16_lossy(&mapped[..written as usize])
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
    resolve_target_with(target, file_filter, |target, file_filter| {
        runtime.browser.resolve_target(target, file_filter)
    })
}

/// Resolve an exact source-navigation target while retaining typed query
/// filtering for query selectors.
pub(crate) fn resolve_source_target(
    runtime: &RuntimeContext,
    target: TargetSelection,
    file_filter: Option<&str>,
) -> Result<ResolvedTarget> {
    resolve_target_with(target, file_filter, |target, file_filter| {
        runtime.browser.resolve_source_target(target, file_filter)
    })
}

fn resolve_target_with(
    target: TargetSelection,
    file_filter: Option<&str>,
    resolve: impl FnOnce(
        codestory_runtime::TargetSelection,
        Option<&str>,
    ) -> std::result::Result<TargetResolution, ApiError>,
) -> Result<ResolvedTarget> {
    let target = match target {
        TargetSelection::Id(id) => codestory_runtime::TargetSelection::Id(id),
        TargetSelection::Query { query, choose } => {
            codestory_runtime::TargetSelection::Query { query, choose }
        }
    };
    match resolve(target, file_filter) {
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
/// Explicit overrides are normalized by native filesystem identity; otherwise
/// the cache root is a stable workspace identity under the process cache root.
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
    let path = match override_dir {
        Some(path) => path.to_path_buf(),
        None => {
            process_cache_root.join(codestory_workspace::workspace_id_v3_for_root(project_root))
        }
    };
    canonicalize_configuration_path(&path)
}

/// Resolve existing configuration roots through native filesystem identity.
/// Missing suffixes retain lexical path rules beneath the nearest existing
/// ancestor so a cache has the same identity before and after creation.
fn canonicalize_configuration_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current working directory")?
            .join(path)
    };
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                lexical.push(component.as_os_str());
            }
        }
    }
    if lexical.exists() {
        return lexical
            .canonicalize()
            .map(normalize_canonical_configuration_path)
            .with_context(|| {
                format!(
                    "Failed to resolve configuration path `{}`",
                    clean_path_string(&lexical.to_string_lossy())
                )
            });
    }

    let mut missing = Vec::new();
    let mut ancestor = lexical.as_path();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        ancestor = parent;
    }
    let mut resolved = if ancestor.exists() {
        ancestor
            .canonicalize()
            .map(normalize_canonical_configuration_path)
            .with_context(|| {
                format!(
                    "Failed to resolve configuration ancestor `{}`",
                    clean_path_string(&ancestor.to_string_lossy())
                )
            })?
    } else {
        ancestor.to_path_buf()
    };
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(windows)]
fn normalize_canonical_configuration_path(path: PathBuf) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    value
        .strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or(path)
}

#[cfg(not(windows))]
fn normalize_canonical_configuration_path(path: PathBuf) -> PathBuf {
    path
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

/// Map runtime API errors into CLI errors with recovery commands when available.
pub(crate) fn map_api_error(error: ApiError) -> anyhow::Error {
    map_api_error_with_project(error, None)
}

/// Map runtime API errors with project-specific recovery guidance.
pub(crate) fn map_api_error_for_project(error: ApiError, project: &Path) -> anyhow::Error {
    map_api_error_with_project(error, Some(project))
}

#[derive(Debug)]
struct CliApiError {
    error: ApiError,
    message: String,
}

impl std::fmt::Display for CliApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliApiError {}

/// Return the typed runtime error retained by the CLI adapter, including when
/// command-specific context has been attached above it.
pub(crate) fn api_error_in_chain(error: &anyhow::Error) -> Option<&ApiError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<CliApiError>())
        .map(|error| &error.error)
}

fn map_api_error_with_project(error: ApiError, project: Option<&Path>) -> anyhow::Error {
    let message = if api_error_is_cache_busy(&error) {
        cache_busy_message(project)
    } else {
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
                message.push_str("\n\nAdditional checks:");
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
        message
    };
    anyhow::Error::new(CliApiError { error, message })
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
        "cache_busy: CodeStory cache is busy or locked. Wait for the active indexing/search process to release the SQLite cache, then retry.\n\nMinimum next:\n  codestory-cli ready --project {project} --goal agent\n\nAdditional checks:\n  codestory-cli ready --project {project} --goal agent\n  codestory-cli doctor --project {project}"
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
        "CODESTORY_EMBED_ALLOW_CPU",
        "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE",
        "CODESTORY_SEMANTIC_DOC_MAX_TOKENS",
        "CODESTORY_STORED_VECTOR_ENCODING",
        "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
    ];
    const HOME_ENV_VARS: &[&str] = &["USERPROFILE", "HOME"];

    struct EnvSnapshot {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    #[test]
    fn api_error_mapping_retains_typed_details_through_outer_context() {
        let expected = ApiError::retrieval_unavailable(
            "retrieval is unavailable",
            "/tmp/project",
            vec!["codestory-cli retrieval index --project /tmp/project".to_string()],
        );
        let mapped = map_api_error(expected.clone()).context("packet activation");

        assert_eq!(api_error_in_chain(&mapped), Some(&expected));
        assert_eq!(mapped.to_string(), "packet activation");
        let human = format!("{mapped:#}");
        assert!(human.contains("retrieval_unavailable: retrieval is unavailable"));
        assert!(human.contains("Minimum next:"));
        assert!(human.contains("codestory-cli retrieval index --project /tmp/project"));
    }

    #[test]
    fn cache_busy_api_error_keeps_type_and_cli_recovery_rendering() {
        let expected = ApiError::new("project_unavailable", "sqlite_busy while opening cache");
        let project = Path::new("/tmp/project");
        let mapped = map_api_error_for_project(expected.clone(), project);

        assert_eq!(api_error_in_chain(&mapped), Some(&expected));
        let human = mapped.to_string();
        let project = quote_command_path(project);
        assert!(human.starts_with("cache_busy: CodeStory cache is busy or locked."));
        assert!(human.contains("Minimum next:"));
        assert!(human.contains(&format!(
            "codestory-cli ready --project {project} --goal agent"
        )));
        assert!(human.contains(&format!("codestory-cli doctor --project {project}")));
    }

    #[test]
    fn public_operation_metadata_preserves_existing_meta_fields() {
        let operation = PublicOperation {
            value: (),
            core_publication: None,
            retrieval_publication: None,
            operation_id: "public-7".to_string(),
            attempt: 2,
        };
        let response = serde_json::json!({
            "result": "ok",
            "_meta": {
                "request_id": "request-1",
                "codestory_publication": {"stale": true}
            }
        });

        let value = public_operation_json_value(&operation, &response)
            .expect("attach canonical publication metadata");

        assert_eq!(
            value.pointer("/_meta/request_id"),
            Some(&serde_json::json!("request-1"))
        );
        assert_eq!(
            value.pointer("/_meta/codestory_publication/served_from"),
            Some(&serde_json::json!("complete_publication"))
        );
        assert_eq!(
            value.pointer("/_meta/codestory_publication/operation/operation_id"),
            Some(&serde_json::json!("public-7"))
        );
        assert_eq!(
            value.pointer("/_meta/codestory_publication/operation/attempt"),
            Some(&serde_json::json!(2))
        );
        assert_eq!(value.pointer("/_meta/codestory_publication/stale"), None);
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
        let expected_cache =
            canonicalize_configuration_path(&cache).expect("trusted cache identity");
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

        assert_eq!(runtime.cache_root, expected_cache);
        assert_eq!(
            runtime.storage_path,
            runtime.cache_root.join("codestory.db")
        );
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
        let expected_cli_cache =
            canonicalize_configuration_path(&cli_cache).expect("CLI cache identity");
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

        assert_eq!(runtime.cache_root, expected_cli_cache);
        assert_eq!(
            runtime.storage_path,
            runtime.cache_root.join("codestory.db")
        );
    }

    #[test]
    fn explicit_startup_snapshots_isolate_concurrent_runtime_paths() {
        let temp = tempdir().expect("temp dir");
        let first_project = temp.path().join("first-project");
        let second_project = temp.path().join("second-project");
        let first_cache = temp.path().join("first-cache");
        let second_cache = temp.path().join("second-cache");
        fs::create_dir_all(&first_project).expect("create first project");
        fs::create_dir_all(&second_project).expect("create second project");
        let first_cache =
            canonicalize_configuration_path(&first_cache).expect("first cache identity");
        let second_cache =
            canonicalize_configuration_path(&second_cache).expect("second cache identity");
        assert_ne!(first_cache, second_cache);
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

        let assert_isolated_paths =
            |runtime: &RuntimeContext, expected_root: &Path, other_root: &Path| {
                assert_eq!(runtime.sidecar.cache_root.as_path(), expected_root);
                assert_eq!(
                    runtime.storage_path,
                    runtime.cache_root.join("codestory.db")
                );
                for path in [
                    runtime.cache_root.as_path(),
                    runtime.storage_path.as_path(),
                    runtime.sidecar.cache_root.as_path(),
                    runtime.sidecar.layout.lexical_data_dir.as_path(),
                    runtime.sidecar.layout.semantic_data_dir.as_path(),
                    runtime.sidecar.layout.scip_artifacts_root.as_path(),
                    runtime.sidecar.layout.state_file.as_path(),
                ] {
                    assert!(
                        path.starts_with(expected_root),
                        "{} must remain under {}",
                        path.display(),
                        expected_root.display()
                    );
                    assert!(
                        !path.starts_with(other_root),
                        "{} must not use {}",
                        path.display(),
                        other_root.display()
                    );
                }
            };
        assert_isolated_paths(&first, &first_cache, &second_cache);
        assert_isolated_paths(&second, &second_cache, &first_cache);
        assert_ne!(
            first.context_key.configuration_id,
            second.context_key.configuration_id
        );
    }

    #[test]
    fn existing_cache_aliases_share_one_configuration_identity() {
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir_all(&project).expect("create project");
        fs::create_dir_all(&cache).expect("create cache");
        let startup = crate::config::CliStartupConfig {
            user_home: None,
            project_network_config_allowed: true,
            stdio_cache_root: Some(temp.path().join("process-cache")),
            sidecar_defaults: codestory_retrieval::SidecarProcessDefaults::new(
                temp.path().join("process-cache"),
                codestory_retrieval::SidecarRuntimeDefaults::default(),
            ),
        };
        let open = |cache_dir: PathBuf| {
            RuntimeContext::new_agent_sidecar_with_startup(
                &ProjectArgs {
                    project: project.clone(),
                    cache_dir: Some(cache_dir),
                },
                &startup,
            )
            .expect("runtime context")
        };

        let canonical = open(cache.clone());
        let dotted = open(cache.join("..").join("cache"));

        assert_eq!(canonical.cache_root, dotted.cache_root);
        assert_eq!(canonical.storage_path, dotted.storage_path);
        assert_eq!(canonical.context_key, dotted.context_key);
    }

    #[test]
    fn explicit_cache_override_is_normalized_without_workspace_hashing() {
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("project");
        let process_cache_root = temp.path().join("process-cache");
        let override_dir = temp.path().join("explicit-cache");
        let expected_override =
            canonicalize_configuration_path(&override_dir).expect("explicit cache identity");
        let default_cache = canonicalize_configuration_path(
            &process_cache_root.join(codestory_workspace::workspace_id_v3_for_root(&project)),
        )
        .expect("default cache identity");

        let actual = cache_root_for_project_in(&project, Some(&override_dir), &process_cache_root)
            .expect("explicit cache root");

        assert_eq!(actual, expected_override);
        assert_ne!(actual, default_cache);
    }

    #[test]
    fn missing_configuration_path_identity_is_stable_after_creation() {
        let temp = tempdir().expect("temp dir");
        let missing = temp.path().join("cache").join("nested");
        let existing_ancestor =
            canonicalize_configuration_path(temp.path()).expect("existing ancestor identity");
        let expected = existing_ancestor.join("cache").join("nested");

        let before = canonicalize_configuration_path(&missing).expect("missing path identity");
        assert_eq!(before, expected);
        fs::create_dir_all(&missing).expect("create configuration path");
        let after = canonicalize_configuration_path(&missing).expect("existing path identity");

        assert_eq!(after, expected);
    }

    #[cfg(windows)]
    #[test]
    fn windows_configuration_identity_converges_case_and_extended_aliases_before_and_after_creation()
     {
        let temp = tempdir().expect("temp dir");
        let mixed = temp.path().join("MissingCache").join("Nested");
        let folded = temp.path().join("missingcache").join("nested");
        let extended = PathBuf::from(format!(r"\\?\{}", folded.display()));

        let before_mixed = canonicalize_configuration_path(&mixed).expect("mixed missing path");
        let before_folded = canonicalize_configuration_path(&folded).expect("folded missing path");
        let before_extended =
            canonicalize_configuration_path(&extended).expect("extended missing path");
        let expected = configuration_path_identity(&before_mixed);
        assert_eq!(configuration_path_identity(&before_folded), expected);
        assert_eq!(configuration_path_identity(&before_extended), expected);

        fs::create_dir_all(&mixed).expect("create mixed-case configuration path");
        let after_mixed = canonicalize_configuration_path(&mixed).expect("mixed existing path");
        let after_folded = canonicalize_configuration_path(&folded).expect("folded existing path");
        let after_extended =
            canonicalize_configuration_path(&extended).expect("extended existing path");
        assert_eq!(configuration_path_identity(&after_mixed), expected);
        assert_eq!(configuration_path_identity(&after_folded), expected);
        assert_eq!(configuration_path_identity(&after_extended), expected);
    }

    #[test]
    fn ordinary_cli_graph_response_retries_the_whole_operation_after_publication_change() {
        let _env_lock = crate::config::config_env_test_lock();
        let _managed_env = EnvSnapshot::clear(MANAGED_ENV_VARS);
        let _home_env = EnvSnapshot::clear(HOME_ENV_VARS);
        unsafe {
            env::set_var("CODESTORY_EMBED_ALLOW_CPU", "1");
        }
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir_all(project.join("src")).expect("create source dir");
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"publication-change-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        let source = project.join("src/lib.rs");
        fs::write(&source, "pub fn pinned_symbol() -> u32 { 1 }\n").expect("write source");
        let args = ProjectArgs {
            project: project.clone(),
            cache_dir: Some(cache),
        };
        let reader = RuntimeContext::new_inspect_only(&args).expect("reader runtime");
        reader
            .ensure_open(RefreshMode::Full)
            .expect("publish initial core generation");
        let publisher = RuntimeContext::new_inspect_only(&args).expect("publisher runtime");
        publisher
            .open_project_summary()
            .expect("bind publisher to existing project");

        let mut attempts = 0_u32;
        let served_generation = reader
            .run_public_operation("graph", || {
                attempts += 1;
                let pinned = reader
                    .public_operation
                    .active_publication()
                    .expect("ordinary CLI response has a core pin")
                    .core_publication
                    .generation;
                if attempts == 1 {
                    fs::write(&source, "pub fn pinned_symbol() -> u32 { 2 }\n")
                        .expect("change source during response construction");
                    publisher
                        .ensure_open(RefreshMode::Full)
                        .expect("publish replacement generation during response construction");
                }
                Ok(pinned)
            })
            .expect("whole ordinary CLI response should retry once")
            .value;
        let current_generation = publisher
            .project
            .complete_index_publication_at(&publisher.storage_path)
            .expect("read current publication")
            .expect("current publication")
            .generation;

        assert_eq!(attempts, 2, "only the complete response may be retried");
        assert_eq!(served_generation, current_generation);
    }

    #[test]
    fn observational_cli_response_allows_stale_source_and_retries_one_core_swap() {
        let _env_lock = crate::config::config_env_test_lock();
        let _managed_env = EnvSnapshot::clear(MANAGED_ENV_VARS);
        let _home_env = EnvSnapshot::clear(HOME_ENV_VARS);
        unsafe {
            env::set_var("CODESTORY_EMBED_ALLOW_CPU", "1");
        }
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir_all(project.join("src")).expect("create source dir");
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"observational-publication-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        let source = project.join("src/lib.rs");
        fs::write(&source, "pub fn pinned_symbol() -> u32 { 1 }\n").expect("write source");
        let args = ProjectArgs {
            project: project.clone(),
            cache_dir: Some(cache),
        };
        let reader = RuntimeContext::new_inspect_only(&args).expect("reader runtime");
        reader
            .ensure_open(RefreshMode::Full)
            .expect("publish initial core generation");
        let publisher = RuntimeContext::new_inspect_only(&args).expect("publisher runtime");
        publisher
            .open_project_summary()
            .expect("bind publisher to existing project");

        fs::write(&source, "pub fn pinned_symbol() -> u32 { 2 }\n")
            .expect("make initial publication stale");
        let mut attempts = 0_u32;
        let operation = reader
            .run_observational_public_operation("affected", || {
                attempts += 1;
                let pinned = reader
                    .public_operation
                    .active_publication()
                    .expect("observational response has a core pin")
                    .core_publication
                    .generation;
                if attempts == 1 {
                    fs::write(&source, "pub fn pinned_symbol() -> u32 { 3 }\n")
                        .expect("change source during response construction");
                    publisher
                        .ensure_open(RefreshMode::Full)
                        .expect("publish replacement generation during response construction");
                }
                Ok(pinned)
            })
            .expect("observational response should retry one core replacement");
        let current_generation = publisher
            .project
            .complete_index_publication_at(&publisher.storage_path)
            .expect("read current publication")
            .expect("current publication")
            .generation;

        assert_eq!(attempts, 2);
        assert_eq!(operation.attempt, 2);
        assert_eq!(operation.value, current_generation);

        let mut churn_attempts = 0_u32;
        let error = reader
            .run_observational_public_operation("affected", || {
                churn_attempts += 1;
                let pinned = reader
                    .public_operation
                    .active_publication()
                    .expect("churning response has a core pin")
                    .core_publication
                    .generation;
                fs::write(
                    &source,
                    format!(
                        "pub fn pinned_symbol() -> u32 {{ {} }}\n",
                        churn_attempts + 3
                    ),
                )
                .expect("change source during every attempt");
                publisher
                    .ensure_open(RefreshMode::Full)
                    .expect("publish a replacement during every attempt");
                Ok(pinned)
            })
            .expect_err("repeated core churn must exhaust the bounded retry");
        assert_eq!(churn_attempts, 2);
        assert!(error.to_string().contains("publication_changed"));

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_build = Arc::clone(&cancelled);
        let mut built = false;
        let error = reader
            .public_operation
            .run_observational_with_cancel("affected", cancelled, || {
                built = true;
                cancelled_for_build.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            })
            .expect_err("cancellation during observational construction must discard the value");
        assert_eq!(error.code, "cancelled");
        assert!(built);
    }

    #[test]
    fn hostile_drill_pin_change_retries_before_building_the_active_summary() {
        let _env_lock = crate::config::config_env_test_lock();
        let _managed_env = EnvSnapshot::clear(MANAGED_ENV_VARS);
        let _home_env = EnvSnapshot::clear(HOME_ENV_VARS);
        unsafe {
            env::set_var("CODESTORY_EMBED_ALLOW_CPU", "1");
        }
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir_all(project.join("src")).expect("create source dir");
        let source = project.join("src/lib.rs");
        fs::write(&source, "// core generation A\n").expect("write source");
        let args = ProjectArgs {
            project: project.clone(),
            cache_dir: Some(cache),
        };
        let reader = RuntimeContext::new_agent_sidecar_with_startup(
            &args,
            &crate::config::process_startup_config(),
        )
        .expect("reader runtime");
        reader
            .ensure_open(RefreshMode::Full)
            .expect("publish core generation A");
        codestory_retrieval::test_support::publish_zero_dense_pinned_query_fixture(
            &project,
            &reader.storage_path,
            &reader.sidecar,
        )
        .expect("publish retrieval generation A");
        let retrieval_a = codestory_retrieval::strict_sidecar_status_for_runtime(
            &project,
            Some(&reader.storage_path),
            reader.sidecar.clone(),
        )
        .expect("inspect retrieval generation A");
        assert!(retrieval_a.is_live_ready(), "{retrieval_a:?}");
        assert!(
            reader.public_operation.retrieval_primary_enabled_for_test(),
            "strict retrieval fixture must be selected by the public operation"
        );
        let generation_a = reader
            .project
            .complete_index_publication_at(&reader.storage_path)
            .expect("read generation A")
            .expect("generation A exists");

        let replacement_generation = generation_a.generation + 1;
        let replacement_generation_id = "22222222-2222-4222-8222-222222222222".to_string();
        let replacement_run_id = "between-pins-run-b".to_string();
        let publisher_project = project.clone();
        let publisher_storage = reader.storage_path.clone();
        let publisher_runtime = reader.sidecar.clone();
        let publisher_generation_id = replacement_generation_id.clone();
        let publisher_run_id = replacement_run_id.clone();
        codestory_runtime::set_before_retrieval_pin_test_hook(move || {
            codestory_retrieval::test_support::publish_replacement_core_and_zero_dense_fixture(
                &publisher_project,
                &publisher_storage,
                &publisher_runtime,
                replacement_generation,
                &publisher_generation_id,
                &publisher_run_id,
            )
            .expect("publish core and retrieval generation B between pins");
        });

        let mut builds = 0_u32;
        let operation = reader
            .run_public_operation("drill", || {
                builds += 1;
                let publication = reader
                    .public_operation
                    .active_publication()
                    .context("drill response has active publications")?;
                let summary = reader.active_project_summary()?;
                Ok((publication, summary))
            })
            .expect("mismatched first pins should retry the complete operation");
        let served_core = operation
            .core_publication
            .as_ref()
            .expect("served core publication");
        assert_eq!(operation.attempt, 2);
        assert_eq!(
            builds, 1,
            "the mismatched attempt must not enter the builder"
        );
        let retrieval_b = codestory_retrieval::strict_sidecar_status_for_runtime(
            &project,
            Some(&reader.storage_path),
            reader.sidecar.clone(),
        )
        .expect("inspect retrieval generation B");
        assert!(retrieval_b.is_live_ready(), "{retrieval_b:?}");
        assert!(reader.public_operation.retrieval_primary_enabled_for_test());
        let served_retrieval = operation
            .retrieval_publication
            .as_ref()
            .expect("served retrieval publication");

        assert_eq!(served_core.generation_id, replacement_generation_id);
        assert_eq!(served_core.run_id, replacement_run_id);
        assert_eq!(
            served_retrieval.core_generation_id,
            served_core.generation_id
        );
        assert_eq!(served_retrieval.core_run_id, served_core.run_id);
        assert_eq!(
            operation.value.0.core_publication.generation_id,
            served_core.generation_id
        );
        assert_eq!(
            operation
                .value
                .0
                .retrieval_publication
                .as_ref()
                .expect("active retrieval publication")
                .core_generation_id,
            served_core.generation_id
        );
        let summary_publication = operation
            .value
            .1
            .publication
            .as_ref()
            .expect("drill active summary publication");
        assert_eq!(summary_publication.generation_id, served_core.generation_id);
        assert_eq!(summary_publication.run_id, served_core.run_id);
        let json = public_operation_json_value(
            &operation,
            &serde_json::json!({"body_publication": summary_publication}),
        )
        .expect("serialize hostile drill response metadata");
        assert_eq!(
            json.pointer("/body_publication/generation_id"),
            json.pointer("/_meta/codestory_publication/core_publication/generation_id")
        );
        assert_eq!(
            json.pointer("/body_publication/run_id"),
            json.pointer("/_meta/codestory_publication/core_publication/run_id")
        );
    }

    #[cfg(unix)]
    #[test]
    fn project_aliases_capture_one_native_workspace_context() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("project");
        let alias = temp.path().join("project-alias");
        let cache = temp.path().join("cache");
        fs::create_dir_all(&project).expect("create project");
        symlink(&project, &alias).expect("create project alias");
        let startup = crate::config::CliStartupConfig {
            user_home: None,
            project_network_config_allowed: true,
            stdio_cache_root: Some(cache.clone()),
            sidecar_defaults: codestory_retrieval::SidecarProcessDefaults::new(
                cache,
                codestory_retrieval::SidecarRuntimeDefaults::default(),
            ),
        };

        let open = |root: PathBuf| {
            RuntimeContext::new_agent_sidecar_with_startup(
                &ProjectArgs {
                    project: root,
                    cache_dir: None,
                },
                &startup,
            )
            .expect("runtime context")
        };
        let canonical = open(project);
        let aliased = open(alias);

        assert!(codestory_workspace::same_workspace_path(
            &canonical.project_root,
            &aliased.project_root
        ));
        assert_eq!(canonical.context_key, aliased.context_key);
        assert_eq!(canonical.cache_root, aliased.cache_root);
        assert_eq!(canonical.storage_path, aliased.storage_path);
    }
}
