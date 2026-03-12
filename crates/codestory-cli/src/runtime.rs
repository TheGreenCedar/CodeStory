use anyhow::{Context, Result, anyhow, bail};
use codestory_contracts::api::{
    ApiError, IndexMode, IndexingPhaseTimings, NodeDetailsDto, NodeDetailsRequest, ProjectSummary,
    SearchHit, SearchRequest,
};
use codestory_runtime::{GroundingService, IndexService, ProjectService, Runtime, SearchService};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

use crate::args::{ProjectArgs, RefreshMode, TargetSelection};
use crate::query_resolution::{
    compare_resolution_hits, resolution_rank, search_hit_matches_file_filter,
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
    pub(crate) project_root: PathBuf,
    pub(crate) storage_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTarget {
    pub(crate) requested: String,
    pub(crate) selected: SearchHit,
    pub(crate) alternatives: Vec<SearchHit>,
}

impl RuntimeContext {
    pub(crate) fn new(args: &ProjectArgs) -> Result<Self> {
        let project_root = canonicalize_project_root(&args.project)?;
        let cache_root = cache_root_for_project(&project_root, args.cache_dir.as_deref())?;
        let storage_path = cache_root.join("codestory.db");
        let runtime = Runtime::new();
        Ok(Self {
            project: runtime.project_service(),
            index: runtime.index_service(),
            search: runtime.search_service(),
            grounding: runtime.grounding_service(),
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
                requested: format!("id:{}", id.0),
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
                    },
                    None,
                    Some(50),
                    None,
                )
                .map_err(map_api_error)?;
            if let Some(file_filter) = file_filter {
                alternatives.retain(|hit| search_hit_matches_file_filter(hit, file_filter));
            }
            if alternatives.is_empty() {
                if let Some(file_filter) = file_filter {
                    return Err(anyhow!(
                        "No symbol matched query `{query}` within files matching `{file_filter}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                    ));
                }
                return Err(anyhow!(
                    "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                ));
            }

            alternatives.sort_by(|left, right| compare_resolution_hits(&query, left, right));

            if alternatives.len() > 1 {
                let rank0 = resolution_rank(&query, &alternatives[0]);
                let rank1 = resolution_rank(&query, &alternatives[1]);
                if rank0 == rank1 {
                    let name0 = &alternatives[0].display_name;
                    let name1 = &alternatives[1].display_name;
                    bail!(
                        "Query `{query}` is ambiguous. It matches multiple distinct symbols equally well, including:\n  • {name0}\n  • {name1}\n\nPlease steer resolution by providing a more qualified name (e.g. `Namespace::Symbol` or a partial path)."
                    );
                }
            }

            let selected = alternatives.first().cloned().ok_or_else(|| {
                if let Some(file_filter) = file_filter {
                    anyhow!(
                        "No symbol matched query `{query}` within files matching `{file_filter}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                    )
                } else {
                    anyhow!(
                        "No symbol matched query `{query}`. Run `codestory-cli search --query \"{query}\"` to inspect candidates."
                    )
                }
            })?;

            Ok(ResolvedTarget {
                requested: query,
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
            project.display()
        )
    })
}

pub(crate) fn cache_root_for_project(
    project_root: &Path,
    override_dir: Option<&Path>,
) -> Result<PathBuf> {
    let base = match override_dir {
        Some(path) => path.to_path_buf(),
        None => ProjectDirs::from("dev", "codestory", "codestory")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .ok_or_else(|| {
                anyhow!("Failed to determine a user cache directory for codestory-cli")
            })?,
    };
    Ok(base.join(fnv1a_hex(project_root.to_string_lossy().as_bytes())))
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
        bail!(
            "No indexed files are available for `{subcommand}`. Run `codestory-cli index --project \"{}\" --refresh auto` first or rerun this command with `--refresh auto`.",
            opened.summary.root
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
    }
}
