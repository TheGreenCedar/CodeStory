use anyhow::{Context, Result, anyhow, bail};
use codestory_contracts::api::{
    ApiError, AppEventPayload, IndexMode, IndexingPhaseTimings, NodeDetailsDto, NodeDetailsRequest,
    ProjectSummary, SearchHit, SearchRepoTextMode, SearchRequest,
};
use codestory_runtime::{
    AgentService, GroundingService, IndexService, ProjectService, Runtime, SearchService,
};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

use crate::args::{ProjectArgs, QuerySelectorOutput, RefreshMode, TargetSelection};
use crate::display::{clean_path_string, format_search_hit_target};
use crate::query_resolution::{
    compare_resolution_hits, file_filter_match_bucket, resolution_rank,
    search_hit_matches_file_filter,
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
    pub(crate) search: SearchService,
    pub(crate) grounding: GroundingService,
    pub(crate) agent: AgentService,
    pub(crate) events: crossbeam_channel::Receiver<AppEventPayload>,
    pub(crate) project_root: PathBuf,
    pub(crate) storage_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTarget {
    pub(crate) selector: QuerySelectorOutput,
    pub(crate) requested: String,
    pub(crate) file_filter: Option<String>,
    pub(crate) selected: SearchHit,
    pub(crate) alternatives: Vec<SearchHit>,
}

impl RuntimeContext {
    pub(crate) fn new(args: &ProjectArgs) -> Result<Self> {
        let project_root = canonicalize_project_root(&args.project)?;
        let config = crate::config::load_config(&project_root)?;
        let cache_root = cache_root_for_project(
            &project_root,
            args.cache_dir.as_deref().or(config.cache_dir.as_deref()),
        )?;
        let storage_path = cache_root.join("codestory.db");
        let runtime = Runtime::new();
        let events = runtime.events();
        Ok(Self {
            project: runtime.project_service(),
            index: runtime.index_service(),
            search: runtime.search_service(),
            grounding: runtime.grounding_service(),
            agent: runtime.agent_service(),
            events,
            project_root,
            storage_path,
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
                .grounding
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
        TargetSelection::Query(query) => {
            let mut alternatives = runtime
                .search
                .search_hybrid(
                    SearchRequest {
                        query: query.clone(),
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
                alternatives.retain(|hit| {
                    search_hit_matches_file_filter(&runtime.project_root, hit, file_filter)
                });
            }
            if alternatives.is_empty() {
                return Err(no_query_match_error(
                    &runtime.project_root,
                    &query,
                    file_filter,
                ));
            }

            alternatives.sort_by(|left, right| {
                compare_resolution_candidates(
                    &runtime.project_root,
                    &query,
                    file_filter,
                    left,
                    right,
                )
            });

            if alternatives.len() > 1 {
                let rank0 = resolution_candidate_rank(
                    &runtime.project_root,
                    &query,
                    file_filter,
                    &alternatives[0],
                );
                let rank1 = resolution_candidate_rank(
                    &runtime.project_root,
                    &query,
                    file_filter,
                    &alternatives[1],
                );
                if rank0 == rank1 {
                    bail!(
                        "{}",
                        ambiguous_query_error(
                            &runtime.project_root,
                            &query,
                            file_filter,
                            &alternatives,
                        )
                    );
                }
            }

            let selected = alternatives
                .first()
                .cloned()
                .ok_or_else(|| no_query_match_error(&runtime.project_root, &query, file_filter))?;

            Ok(ResolvedTarget {
                selector: QuerySelectorOutput::Query,
                requested: query,
                file_filter: file_filter.map(ToOwned::to_owned),
                selected,
                alternatives,
            })
        }
    }
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
                .ok_or_else(|| {
                    anyhow!("Failed to determine a user cache directory for codestory-cli")
                })?;
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
        resolvable: true,
        score_breakdown: None,
    }
}

fn resolution_candidate_rank(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    hit: &SearchHit,
) -> (u8, u8, u8, u8, u8, u8) {
    let rank = resolution_rank(query, hit);
    (
        file_filter
            .map(|filter| file_filter_match_bucket(project_root, hit, filter))
            .unwrap_or(0),
        rank.0,
        rank.1,
        rank.2,
        rank.3,
        rank.4,
    )
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
}

fn no_query_match_error(
    _project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
) -> anyhow::Error {
    match file_filter {
        Some(file_filter) => anyhow!(
            "No symbol matched query `{query}` within files matching `{}`. Run `codestory-cli search --query \"{query}\" --limit 10` to inspect candidates, or relax `--file`.",
            clean_path_string(file_filter)
        ),
        None => anyhow!(
            "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\" --limit 10` to inspect candidates."
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
    for hit in alternatives.iter().take(4) {
        message.push_str("  - ");
        message.push_str(&format_search_hit_target(project_root, hit));
        message.push('\n');
    }
    if file_filter.is_some() {
        message.push_str(
            "\nPlease pass a more qualified symbol name or an exact `--id` from `search` output.",
        );
    } else {
        message.push_str(
            "\nPlease pass a more qualified symbol name, add `--file <path-fragment>`, or resolve the exact `--id` from `search` output.",
        );
    }
    message
}
