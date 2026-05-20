use anyhow::{Context, Result, anyhow, bail};
use codestory_contracts::api::{
    ApiError, AppEventPayload, IndexMode, IndexingPhaseTimings, NodeDetailsDto, NodeDetailsRequest,
    ProjectSummary, SearchHit, SearchRepoTextMode, SearchRequest,
};
use codestory_runtime::{
    BookmarkService, GroundingService, IndexService, ProjectService, ReadOnlyBrowserService,
    Runtime,
};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

use crate::args::{ProjectArgs, QuerySelectorOutput, RefreshMode, TargetSelection};
use crate::display::{clean_path_string, format_search_hit_target, relative_path};
use crate::query_resolution::{
    ResolutionRank, compare_resolution_hits, file_filter_match_bucket,
    resolution_rank_with_project_root, search_hit_matches_file_filter,
};

#[derive(Debug)]
pub(crate) struct OpenedProject {
    pub(crate) summary: ProjectSummary,
    pub(crate) refresh_mode: Option<IndexMode>,
    pub(crate) phase_timings: Option<IndexingPhaseTimings>,
}

pub(crate) struct RuntimeContext {
    pub(crate) project: ProjectService,
    pub(crate) index: IndexService,
    pub(crate) grounding: GroundingService,
    pub(crate) bookmarks: BookmarkService,
    pub(crate) browser: ReadOnlyBrowserService,
    pub(crate) events: crossbeam_channel::Receiver<AppEventPayload>,
    pub(crate) project_root: PathBuf,
    pub(crate) storage_path: PathBuf,
    pub(crate) managed_embeddings_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ResolutionCandidateRank {
    file_filter_match: u8,
    resolution: ResolutionRank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedEmbeddingStartup {
    AutostartIfInstalled,
    InspectOnly,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTarget {
    pub(crate) selector: QuerySelectorOutput,
    pub(crate) requested: String,
    pub(crate) file_filter: Option<String>,
    pub(crate) selected: SearchHit,
    pub(crate) alternatives: Vec<SearchHit>,
}

#[derive(Debug, Clone)]
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
    pub(crate) fn new(args: &ProjectArgs) -> Result<Self> {
        Self::new_with_startup(args, ManagedEmbeddingStartup::AutostartIfInstalled)
    }

    pub(crate) fn new_inspect_only(args: &ProjectArgs) -> Result<Self> {
        Self::new_with_startup(args, ManagedEmbeddingStartup::InspectOnly)
    }

    fn new_with_startup(args: &ProjectArgs, _startup: ManagedEmbeddingStartup) -> Result<Self> {
        let project_root = canonicalize_project_root(&args.project)?;
        let config = crate::config::load_config(&project_root)?;
        let cache_override = args.cache_dir.as_deref().or(config.cache_dir.as_deref());
        let cache_root = cache_root_for_project(&project_root, cache_override)?;
        let managed_embeddings_root =
            crate::managed_embeddings::managed_root(args.cache_dir.as_deref())?;
        crate::managed_embeddings::prepare_runtime_if_installed(&managed_embeddings_root);
        let storage_path = cache_root.join("codestory.db");
        let runtime = Runtime::new();
        let events = runtime.events();
        Ok(Self {
            project: runtime.project_service(),
            index: runtime.index_service(),
            grounding: runtime.grounding_service(),
            bookmarks: runtime.bookmark_service(),
            browser: runtime.browser_service(),
            events,
            project_root,
            storage_path,
            managed_embeddings_root,
        })
    }

    pub(crate) fn ensure_open(&self, refresh: RefreshMode) -> Result<OpenedProject> {
        let mut summary = self.open_project_summary()?;
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

    pub(crate) fn open_project_summary(&self) -> Result<ProjectSummary> {
        self.project
            .open_project_summary_with_storage_path(
                self.project_root.clone(),
                self.storage_path.clone(),
            )
            .map_err(map_api_error)
    }
}

pub(crate) fn resolve_target(
    runtime: &RuntimeContext,
    target: TargetSelection,
    file_filter: Option<&str>,
) -> Result<ResolvedTarget> {
    match target {
        TargetSelection::Id(id) => {
            let details = runtime
                .browser
                .node_details(NodeDetailsRequest { id: id.clone() })
                .map_err(map_api_error)?;
            Ok(ResolvedTarget {
                selector: QuerySelectorOutput::Id,
                requested: id.0,
                file_filter: None,
                selected: search_hit_from_node(&details),
                alternatives: Vec::new(),
            })
        }
        TargetSelection::Query { query, choose } => {
            resolve_query_target(runtime, query, choose, file_filter)
        }
    }
}

fn resolve_query_target(
    runtime: &RuntimeContext,
    query: String,
    choose: Option<usize>,
    file_filter: Option<&str>,
) -> Result<ResolvedTarget> {
    let alternatives = query_resolution_alternatives(runtime, &query, file_filter)?;
    let tied_alternatives =
        tied_top_alternatives(&runtime.project_root, &query, file_filter, &alternatives);

    if let Some(choice) = choose {
        return resolve_chosen_query_target(
            query,
            file_filter,
            alternatives,
            tied_alternatives,
            choice,
        );
    }

    reject_ambiguous_query_target(
        &runtime.project_root,
        &query,
        file_filter,
        tied_alternatives,
    )?;
    debug_assert_unique_top_candidate(&runtime.project_root, &query, file_filter, &alternatives);

    let selected = alternatives
        .first()
        .cloned()
        .ok_or_else(|| no_query_match_error(&runtime.project_root, &query, file_filter))?;
    Ok(query_resolved_target(
        query,
        file_filter,
        selected,
        alternatives,
    ))
}

fn query_resolution_alternatives(
    runtime: &RuntimeContext,
    query: &str,
    file_filter: Option<&str>,
) -> Result<Vec<SearchHit>> {
    let mut alternatives = runtime
        .browser
        .search_hybrid(
            SearchRequest {
                query: query.to_owned(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 50,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            None,
            Some(50),
            None,
        )
        .map_err(map_api_error)?;
    if let Some(file_filter) = file_filter {
        alternatives
            .retain(|hit| search_hit_matches_file_filter(&runtime.project_root, hit, file_filter));
    }
    if alternatives.is_empty() {
        return Err(no_query_match_error(
            &runtime.project_root,
            query,
            file_filter,
        ));
    }

    alternatives.sort_by(|left, right| {
        compare_resolution_candidates(&runtime.project_root, query, file_filter, left, right)
    });
    Ok(alternatives)
}

fn resolve_chosen_query_target(
    query: String,
    file_filter: Option<&str>,
    mut alternatives: Vec<SearchHit>,
    tied_alternatives: Vec<SearchHit>,
    choice: usize,
) -> Result<ResolvedTarget> {
    if choice == 0 || choice > tied_alternatives.len() {
        bail!(
            "`--choose {choice}` is outside the displayed alternative range 1..={}. Re-run without `--choose` to inspect the current alternatives.",
            tied_alternatives.len()
        );
    }

    let selected = tied_alternatives[choice - 1].clone();
    promote_selected_alternative(&mut alternatives, &selected);
    Ok(query_resolved_target(
        query,
        file_filter,
        selected,
        alternatives,
    ))
}

fn promote_selected_alternative(alternatives: &mut Vec<SearchHit>, selected: &SearchHit) {
    if let Some(position) = alternatives
        .iter()
        .position(|hit| hit.node_id == selected.node_id)
    {
        let chosen = alternatives.remove(position);
        alternatives.insert(0, chosen);
    }
}

fn reject_ambiguous_query_target(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    tied_alternatives: Vec<SearchHit>,
) -> Result<()> {
    if tied_alternatives.len() <= 1 {
        return Ok(());
    }

    let message = ambiguous_query_error(project_root, query, file_filter, &tied_alternatives);
    Err(AmbiguousTargetError {
        query: query.to_owned(),
        file_filter: file_filter.map(ToOwned::to_owned),
        alternatives: tied_alternatives,
        message,
    }
    .into())
}

fn debug_assert_unique_top_candidate(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    alternatives: &[SearchHit],
) {
    if alternatives.len() > 1 {
        let rank1 = resolution_candidate_rank(project_root, query, file_filter, &alternatives[1]);
        debug_assert_ne!(
            resolution_candidate_rank(project_root, query, file_filter, &alternatives[0]),
            rank1
        );
    }
}

fn query_resolved_target(
    query: String,
    file_filter: Option<&str>,
    selected: SearchHit,
    alternatives: Vec<SearchHit>,
) -> ResolvedTarget {
    ResolvedTarget {
        selector: QuerySelectorOutput::Query,
        requested: query,
        file_filter: file_filter.map(ToOwned::to_owned),
        selected,
        alternatives,
    }
}

fn tied_top_alternatives(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    alternatives: &[SearchHit],
) -> Vec<SearchHit> {
    let Some(first) = alternatives.first() else {
        return Vec::new();
    };
    let top_rank = resolution_candidate_rank(project_root, query, file_filter, first);
    alternatives
        .iter()
        .take_while(|hit| {
            resolution_candidate_rank(project_root, query, file_filter, hit) == top_rank
        })
        .cloned()
        .collect()
}

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

pub(crate) fn cache_root_for_project(
    project_root: &Path,
    override_dir: Option<&Path>,
) -> Result<PathBuf> {
    match override_dir {
        Some(path) => Ok(path.to_path_buf()),
        None => {
            let base = ProjectDirs::from("dev", "codestory", "codestory")
                .map(|dirs| dirs.cache_dir().to_path_buf())
                .unwrap_or_else(|| std::env::temp_dir().join("codestory").join("cache"));
            Ok(base.join(fnv1a_hex(project_root.to_string_lossy().as_bytes())))
        }
    }
}

pub(crate) fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn project_config_cache_dir_does_not_select_managed_executable_root() {
        let temp = tempdir().expect("temp dir");
        let project = temp.path().join("repo");
        let config_cache = temp.path().join("repo-controlled-cache");
        fs::create_dir_all(&project).expect("create project");
        fs::write(
            project.join(".codestory.toml"),
            format!("cache_dir = {:?}\n", config_cache.to_string_lossy()),
        )
        .expect("write project config");

        let context = RuntimeContext::new_inspect_only(&ProjectArgs {
            project,
            cache_dir: None,
        })
        .expect("runtime context");

        assert_eq!(context.storage_path, config_cache.join("codestory.db"));
        assert_ne!(
            context.managed_embeddings_root,
            config_cache.join("managed-embeddings"),
            "repo-controlled config cache_dir must not choose executable managed asset root"
        );
    }
}

pub(crate) fn resolve_refresh_request(
    refresh: RefreshMode,
    summary: &ProjectSummary,
) -> Option<IndexMode> {
    match refresh {
        RefreshMode::Auto => Some(if summary.stats.node_count == 0 {
            IndexMode::Full
        } else {
            IndexMode::Incremental
        }),
        RefreshMode::Full => Some(IndexMode::Full),
        RefreshMode::Incremental => Some(IndexMode::Incremental),
        RefreshMode::None => None,
    }
}

pub(crate) fn refresh_label(requested: RefreshMode, resolved: Option<IndexMode>) -> String {
    match (requested, resolved) {
        (RefreshMode::Auto, Some(IndexMode::Full)) => "auto(full)".to_string(),
        (RefreshMode::Auto, Some(IndexMode::Incremental)) => "auto(incremental)".to_string(),
        (RefreshMode::Full, Some(_)) => "full".to_string(),
        (RefreshMode::Incremental, Some(_)) => "incremental".to_string(),
        (RefreshMode::None, None) => "none".to_string(),
        _ => "unknown".to_string(),
    }
}

pub(crate) fn ensure_index_ready(opened: &OpenedProject, subcommand: &str) -> Result<()> {
    if opened.summary.stats.node_count == 0 {
        let project = clean_path_string(&opened.summary.root);
        bail!(
            "No indexed files are available for `{subcommand}` in `{project}`.\n\n`{subcommand}` only reads the existing cache unless you pass `--refresh`.\nRun `codestory-cli index --project \"{project}\" --refresh full` to build the cache first.\nIf you want the read command to refresh on demand, rerun it with `--refresh incremental` or `--refresh full`."
        );
    }
    Ok(())
}

pub(crate) fn map_api_error(error: ApiError) -> anyhow::Error {
    anyhow!("{}: {}", error.code, error.message)
}

pub(crate) fn search_hit_from_node(node: &NodeDetailsDto) -> SearchHit {
    SearchHit {
        node_id: node.id.clone(),
        display_name: node.display_name.clone(),
        kind: node.kind,
        file_path: node.file_path.clone(),
        line: node.start_line,
        score: 0.0,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        score_breakdown: None,
    }
}

fn resolution_candidate_rank(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    hit: &SearchHit,
) -> ResolutionCandidateRank {
    let rank = resolution_rank_with_project_root(Some(project_root), query, hit);
    ResolutionCandidateRank {
        file_filter_match: file_filter
            .map(|filter| file_filter_match_bucket(project_root, hit, filter))
            .unwrap_or(0),
        resolution: rank,
    }
}

fn compare_resolution_candidates(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    left: &SearchHit,
    right: &SearchHit,
) -> std::cmp::Ordering {
    resolution_candidate_rank(project_root, query, file_filter, right)
        .cmp(&resolution_candidate_rank(
            project_root,
            query,
            file_filter,
            left,
        ))
        .then_with(|| compare_resolution_hits(query, left, right))
        .then_with(|| left.node_id.0.cmp(&right.node_id.0))
}

fn no_query_match_error(
    _project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
) -> anyhow::Error {
    match file_filter {
        Some(file_filter) => anyhow!(
            "query_resolution: No symbol matched query `{query}` within files matching `{}`. Run `codestory-cli search --query \"{query}\" --limit 10` to inspect candidates, or relax `--file`.",
            clean_path_string(file_filter)
        ),
        None => anyhow!(
            "query_resolution: No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\" --limit 10` to inspect candidates."
        ),
    }
}

fn ambiguous_query_error(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    alternatives: &[SearchHit],
) -> String {
    let mut message = String::new();
    let scope = file_filter
        .map(|value| format!(" even after applying `--file {}`", clean_path_string(value)))
        .unwrap_or_default();
    message.push_str(&format!(
        "Query `{query}` is ambiguous{scope}. Top equally ranked matches:\n"
    ));
    for (index, hit) in alternatives.iter().enumerate() {
        let number = index + 1;
        message.push_str("  ");
        message.push_str(&number.to_string());
        message.push_str(". ");
        message.push_str(&format_search_hit_target(project_root, hit));
        message.push_str(" id=`");
        message.push_str(&hit.node_id.0);
        message.push('`');
        if let Some(node_ref) = node_ref(project_root, hit) {
            message.push_str(" ref=`");
            message.push_str(&node_ref);
            message.push('`');
        }
        message.push('\n');
    }
    message.push_str("\nNext commands:\n");
    message.push_str(&format!(
        "  codestory-cli symbol --project {} --query {}{} --choose 1\n",
        quote_cli_path(project_root),
        quote_cli_value(query),
        file_filter
            .map(|value| format!(" --file {}", quote_cli_value(&clean_path_string(value))))
            .unwrap_or_default()
    ));
    if let Some(first) = alternatives.first() {
        message.push_str(&format!(
            "  codestory-cli symbol --project {} --id {}\n",
            quote_cli_path(project_root),
            first.node_id.0
        ));
        if let Some(path) = first.file_path.as_deref() {
            message.push_str(&format!(
                "  codestory-cli symbol --project {} --query {} --file {}\n",
                quote_cli_path(project_root),
                quote_cli_value(query),
                quote_cli_value(&relative_path(project_root, path))
            ));
        }
    }
    if file_filter.is_some() {
        message.push_str(
            "\nPass a more qualified symbol name, a stable `--id`, or a narrower `--file` fragment.",
        );
    } else {
        message.push_str(
            "\nPass a more qualified symbol name, add `--file <path-fragment>`, or resolve the exact `--id` from `search` output.",
        );
    }
    message
}

fn node_ref(project_root: &Path, hit: &SearchHit) -> Option<String> {
    let file_path = hit.file_path.as_deref()?;
    let line = hit.line?;
    Some(format!(
        "{}:{line}:{}",
        relative_path(project_root, file_path),
        hit.display_name
    ))
}

fn quote_cli_path(path: &Path) -> String {
    quote_cli_value(&clean_path_string(&path.to_string_lossy()))
}

fn quote_cli_value(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}
