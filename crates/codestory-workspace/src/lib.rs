//! Workspace discovery and refresh planning for the indexing pipeline.
//!
//! This crate turns a repository root plus optional CodeStory manifests into a
//! stable set of source paths. It does not parse code and it does not persist
//! graph state; its contract is to decide which files are in scope, which
//! stored file records are stale, and which projections should be removed.
//!
//! Freshness uses paths, mtimes, and verified parser content hashes when they
//! are available. Callers must provide inventory from the same project root
//! they are planning. Discovery completeness is explicit so an unreadable or
//! bounded walk cannot be mistaken for file deletion.

use anyhow::{Result, bail};
pub use codestory_contracts::workspace::{
    BuildMode, IndexedFileRecord, RefreshExecutionPlan, RefreshInfo, RefreshInputs, RefreshMode,
    RefreshPlan, StoredFileRecord, StoredFileState, WorkspaceInventory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub mod atomic_file;
pub mod owned_deletion;
mod repository_identity;
pub use repository_identity::{
    PROJECT_IDENTITY_SCHEMA_VERSION, PROJECT_IDENTITY_V3_SCHEMA_VERSION, ProjectIdentityV2,
    ProjectIdentityV3, REPOSITORY_IDENTITY_SCHEMA_VERSION, REPOSITORY_IDENTITY_V2_SCHEMA_VERSION,
    RepositoryIdentity, RepositoryIdentityV2, SidecarProjectIdentity, cached_project_identity_v2,
    cached_project_identity_v3, inspect_repository_identity, inspect_repository_identity_v2,
    project_identity_v2, project_identity_v3, project_identity_v3_from_repository,
    same_workspace_path, sidecar_project_identity, workspace_id_for_root, workspace_id_v3_for_root,
};

/// Source-group language selector used during workspace discovery.
///
/// Parser support is defined by the shared language-support registry. Some
/// variants are admitted as structural or text evidence only; matching a
/// `Language` here means the file can enter a refresh plan, not that the
/// indexer will emit parser-backed graph edges for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Language {
    Cxx,
    Java,
    Python,
    Rust,
    JavaScript,
    TypeScript,
    Go,
    Ruby,
    Php,
    CSharp,
    Kotlin,
    Swift,
    Dart,
    Lua,
    Sql,
    Html,
    Css,
    Bash,
    PowerShell,
    Svelte,
    Vue,
    Astro,
}

/// Optional language standard metadata carried by manifests.
///
/// Discovery preserves this value for downstream consumers. The workspace
/// layer does not validate compiler flags or infer support tiers from it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LanguageStandard {
    Default,
    Cxx11,
    Cxx14,
    Cxx17,
    Cxx20,
    Java8,
    Java11,
    Java17,
}

/// Serializable CodeStory project settings.
///
/// `source_groups` are the roots and filters used by discovery. A stored
/// manifest with explicit groups filters by language; a synthetic default
/// manifest keeps all supported paths so mixed-language repositories can be
/// indexed without a hand-written config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub name: String,
    pub version: u32,
    pub source_groups: Vec<SourceGroupSettings>,
}

/// One discovered source group in a project manifest.
///
/// `source_paths` may be files or directories relative to the manifest root.
/// Programmatic callers that construct a trusted manifest can also opt into
/// absolute or outside-root paths. `exclude_patterns` are applied against both
/// workspace-relative and source-root-relative paths so repo-local build output
/// can be pruned without excluding an explicitly selected workspace under a
/// directory such as `target`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceGroupSettings {
    pub id: Uuid,
    pub language: Language,
    pub standard: LanguageStandard,
    pub source_paths: Vec<PathBuf>,
    pub exclude_patterns: Vec<String>,
    pub include_paths: Vec<PathBuf>,
    pub defines: HashMap<String, String>,
    pub language_specific: LanguageSpecificSettings,
}

/// Language-specific discovery metadata carried through the manifest.
///
/// These settings describe caller intent for later stages. Discovery itself
/// only uses the source paths, excludes, and `Language` selector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LanguageSpecificSettings {
    Cxx {
        cdb_path: Option<PathBuf>,
        header_paths: Vec<PathBuf>,
        precompiled_header: Option<PathBuf>,
    },
    Java {
        classpath: Vec<PathBuf>,
        maven_path: Option<PathBuf>,
        gradle_path: Option<PathBuf>,
    },
    Python {
        python_path: Option<PathBuf>,
        virtual_env: Option<PathBuf>,
    },
    Other,
}

/// Loaded project manifest plus discovery state.
///
/// `WorkspaceManifest::open` prefers `codestory_workspace.json`, then
/// `codestory_project.json`, then a synthetic default rooted at the requested
/// directory. Synthetic manifests are intentionally broad: they keep
/// non-Rust files so structural collectors and parser-backed indexers can make
/// the final evidence-tier decision.
#[derive(Debug, Clone)]
pub struct WorkspaceManifest {
    settings: WorkspaceSettings,
    manifest_path: PathBuf,
    is_synthetic_default: Cell<bool>,
    trusted_source_paths: Cell<bool>,
    members: Vec<PathBuf>,
}

/// Multi-member workspace manifest.
///
/// Each member becomes a synthetic source group rooted at that member path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMembersManifest {
    pub members: Vec<PathBuf>,
}

/// Stateless discovery and refresh-planning facade.
///
/// Use this when the caller already has a manifest and stored inventory. The
/// methods are pure with respect to CodeStory storage: they inspect the
/// filesystem, compare stored mtimes, content hashes, and ids, and return a
/// plan for the caller to execute.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDiscovery;

/// Whether discovery observed the entire configured workspace inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceInventoryOutcome {
    Complete,
    Partial,
    Unreadable,
    Bounded,
}

impl WorkspaceInventoryOutcome {
    pub fn is_complete(self) -> bool {
        self == Self::Complete
    }
}

/// One discovery failure retained instead of being treated as an absent file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInventoryIssue {
    pub path: PathBuf,
    pub message: String,
}

/// Current source candidates plus proof of inventory completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFileInventory {
    pub files: Vec<PathBuf>,
    pub outcome: WorkspaceInventoryOutcome,
    pub issues: Vec<WorkspaceInventoryIssue>,
}

/// Refresh plan paired with the inventory outcome that made deletion safe or unsafe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRefreshOutcome {
    pub plan: RefreshPlan,
    pub inventory_outcome: WorkspaceInventoryOutcome,
    pub inventory_issues: Vec<WorkspaceInventoryIssue>,
}

#[derive(Debug, Clone)]
struct CompiledExcludePattern {
    raw: String,
    patterns: Vec<glob::Pattern>,
    match_absolute: bool,
}

impl WorkspaceManifest {
    /// Build a manifest from already-parsed settings.
    ///
    /// The manifest is treated as explicit configuration, so discovery filters
    /// files by each source group's `Language`.
    pub fn from_parts(settings: WorkspaceSettings, manifest_path: PathBuf) -> Self {
        Self {
            settings,
            manifest_path,
            is_synthetic_default: Cell::new(false),
            trusted_source_paths: Cell::new(true),
            members: Vec::new(),
        }
    }

    /// Load a `codestory_project.json` manifest from disk.
    pub fn load(path: PathBuf) -> Result<Self> {
        let content = fs::read_to_string(&path)?;
        let settings: WorkspaceSettings = serde_json::from_str(&content)?;
        Ok(Self {
            settings,
            manifest_path: path,
            is_synthetic_default: Cell::new(false),
            trusted_source_paths: Cell::new(false),
            members: Vec::new(),
        })
    }

    /// Persist explicit project settings to `manifest_path`.
    ///
    /// Saving clears the synthetic-default marker, so subsequent discovery uses
    /// the stored source-group language filters.
    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.settings)?;
        fs::write(&self.manifest_path, content)?;
        self.is_synthetic_default.set(false);
        Ok(())
    }

    /// Create an empty explicit project manifest.
    pub fn new(name: String, manifest_path: PathBuf) -> Self {
        Self {
            settings: WorkspaceSettings {
                name,
                version: 1,
                source_groups: Vec::new(),
            },
            manifest_path,
            is_synthetic_default: Cell::new(false),
            trusted_source_paths: Cell::new(false),
            members: Vec::new(),
        }
    }

    /// Open a repository root as a CodeStory workspace.
    ///
    /// Existing workspace/project manifests are honored. Without one, a
    /// synthetic default manifest is returned; that fallback is intentionally
    /// not a persisted configuration until `save` is called.
    pub fn open(root_path: PathBuf) -> Result<Self> {
        let workspace_file = root_path.join("codestory_workspace.json");
        if workspace_file.exists() {
            return Self::load_workspace_members(root_path, workspace_file);
        }

        let project_file = root_path.join("codestory_project.json");
        if project_file.exists() {
            Self::load(project_file)
        } else {
            let mut manifest = Self::new(
                root_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Project")
                    .to_string(),
                project_file,
            );
            manifest.settings.source_groups.push(SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Rust,
                standard: LanguageStandard::Default,
                source_paths: vec![root_path],
                exclude_patterns: vec![
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.git/**".to_string(),
                    "**/dist/**".to_string(),
                    "**/build/**".to_string(),
                ],
                include_paths: Vec::new(),
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            });
            manifest.is_synthetic_default.set(true);
            manifest.trusted_source_paths.set(true);
            Ok(manifest)
        }
    }

    fn load_workspace_members(root_path: PathBuf, manifest_path: PathBuf) -> Result<Self> {
        let content = fs::read_to_string(&manifest_path)?;
        let workspace: WorkspaceMembersManifest = serde_json::from_str(&content)?;
        let mut manifest = Self::new(
            root_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Workspace")
                .to_string(),
            manifest_path,
        );
        manifest.members = workspace.members.clone();
        manifest.is_synthetic_default.set(true);
        manifest.settings.source_groups = workspace
            .members
            .iter()
            .map(|member| SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Rust,
                standard: LanguageStandard::Default,
                source_paths: vec![member.clone()],
                exclude_patterns: vec![
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.git/**".to_string(),
                    "**/dist/**".to_string(),
                    "**/build/**".to_string(),
                ],
                include_paths: Vec::new(),
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            })
            .collect();
        manifest.trusted_source_paths.set(false);
        Ok(manifest)
    }

    /// Return the effective manifest settings used by discovery.
    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    /// Return the manifest path that defines the workspace root.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Return the directory used for relative source paths and inventory keys.
    pub fn root_dir(&self) -> PathBuf {
        self.manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }

    /// Return member roots loaded from `codestory_workspace.json`.
    pub fn members(&self) -> &[PathBuf] {
        &self.members
    }

    fn should_filter_source_group_language(&self) -> bool {
        !self.is_synthetic_default.get()
    }

    /// Discover all currently in-scope source files.
    ///
    /// Results are normalized, de-duplicated, and sorted. Symlinked directory
    /// entries that escape the selected source root are rejected.
    pub fn source_files(&self) -> Result<Vec<PathBuf>> {
        WorkspaceDiscovery.source_files(self)
    }

    /// Discover source files unless the candidate set exceeds `max_files`.
    ///
    /// Returns `Ok(None)` when the bound is exceeded, allowing callers to avoid
    /// expensive planning in large or unexpected workspaces.
    pub fn source_files_bounded(&self, max_files: usize) -> Result<Option<Vec<PathBuf>>> {
        WorkspaceDiscovery.source_files_bounded(self, max_files)
    }

    /// Discover source files while preserving completeness and failures.
    pub fn source_inventory(&self) -> Result<WorkspaceFileInventory> {
        WorkspaceDiscovery.source_inventory(self)
    }

    /// Build a full-refresh plan that indexes every currently discovered file.
    pub fn full_refresh_plan(&self) -> Result<RefreshPlan> {
        Ok(RefreshPlan {
            mode: RefreshMode::FullRefresh,
            files_to_index: self.source_files()?,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        })
    }

    /// Back-compatible alias for `full_refresh_plan`.
    pub fn full_refresh_execution_plan(&self) -> Result<RefreshPlan> {
        self.full_refresh_plan()
    }

    /// Build an incremental refresh plan from stored file inventory.
    ///
    /// A file is scheduled when it is new, unreadable, previously unindexed,
    /// carries a retryable file-level error, its filesystem mtime differs from
    /// the stored millisecond timestamp, or its verified parser content hash
    /// no longer matches.
    /// Stored file ids absent from current discovery are scheduled for removal.
    pub fn build_execution_plan(&self, inputs: &RefreshInputs) -> Result<RefreshPlan> {
        WorkspaceDiscovery.build_refresh_plan(self, inputs)
    }

    /// Build an incremental plan and retain the discovery outcome behind it.
    pub fn build_execution_outcome(
        &self,
        inputs: &RefreshInputs,
    ) -> Result<WorkspaceRefreshOutcome> {
        WorkspaceDiscovery.build_refresh_outcome(self, inputs)
    }

    /// Build an incremental refresh plan with a current-file discovery bound.
    ///
    /// Returns `Ok(None)` when discovery exceeds `max_current_files`.
    pub fn build_execution_plan_bounded(
        &self,
        inputs: &RefreshInputs,
        max_current_files: usize,
    ) -> Result<Option<RefreshPlan>> {
        WorkspaceDiscovery.build_refresh_plan_bounded(self, inputs, max_current_files)
    }

    /// Build a bounded plan without discarding why the inventory was incomplete.
    pub fn build_execution_outcome_bounded(
        &self,
        inputs: &RefreshInputs,
        max_current_files: usize,
    ) -> Result<WorkspaceRefreshOutcome> {
        WorkspaceDiscovery.build_refresh_outcome_inner(self, inputs, Some(max_current_files))
    }
}

impl WorkspaceDiscovery {
    /// Discover all source files for `manifest`.
    pub fn source_files(&self, manifest: &WorkspaceManifest) -> Result<Vec<PathBuf>> {
        let inventory = self.source_inventory(manifest)?;
        if !inventory.outcome.is_complete() {
            bail!(inventory_failure_message(&inventory));
        }
        Ok(inventory.files)
    }

    /// Discover source files with a hard candidate-count bound.
    pub fn source_files_bounded(
        &self,
        manifest: &WorkspaceManifest,
        max_files: usize,
    ) -> Result<Option<Vec<PathBuf>>> {
        let inventory = self.source_inventory_inner(manifest, Some(max_files))?;
        match inventory.outcome {
            WorkspaceInventoryOutcome::Complete => Ok(Some(inventory.files)),
            WorkspaceInventoryOutcome::Bounded => Ok(None),
            WorkspaceInventoryOutcome::Partial | WorkspaceInventoryOutcome::Unreadable => {
                bail!(inventory_failure_message(&inventory))
            }
        }
    }

    /// Discover source files while retaining traversal failures.
    pub fn source_inventory(&self, manifest: &WorkspaceManifest) -> Result<WorkspaceFileInventory> {
        self.source_inventory_inner(manifest, None)
    }

    fn source_inventory_inner(
        &self,
        manifest: &WorkspaceManifest,
        max_files: Option<usize>,
    ) -> Result<WorkspaceFileInventory> {
        let workspace_root = workspace_root(manifest);
        let mut all_files = Vec::new();
        let mut seen = HashSet::new();
        let mut issues = Vec::new();
        let mut inspected_source_roots = 0usize;

        for group in &manifest.settings.source_groups {
            let exclude_patterns = compile_exclude_patterns(&group.exclude_patterns)?;
            let filter_by_language = manifest.should_filter_source_group_language();
            for source_path in &group.source_paths {
                let full_path = resolve_manifest_source_path(manifest, source_path)?;
                let source_root = discovery_root(&full_path);

                let metadata = match fs::metadata(&full_path) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        issues.push(WorkspaceInventoryIssue {
                            path: full_path,
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                if !metadata.is_file() && !metadata.is_dir() {
                    issues.push(WorkspaceInventoryIssue {
                        path: full_path,
                        message: "unsupported source root file type".to_string(),
                    });
                    continue;
                }
                inspected_source_roots += 1;

                if metadata.is_file() {
                    if !should_include_discovered_path(
                        &full_path,
                        false,
                        &workspace_root,
                        &source_root,
                        filter_by_language,
                        &group.language,
                        &exclude_patterns,
                    ) {
                        continue;
                    }
                    if !push_discovered_file_within_limit(
                        &mut all_files,
                        &mut seen,
                        full_path,
                        &workspace_root,
                        max_files,
                    ) {
                        all_files.sort();
                        return Ok(WorkspaceFileInventory {
                            files: all_files,
                            outcome: WorkspaceInventoryOutcome::Bounded,
                            issues,
                        });
                    }
                    continue;
                }
                if metadata.is_dir() {
                    let mut builder = ignore::WalkBuilder::new(&full_path);
                    builder.follow_links(true);
                    builder.require_git(false);
                    let workspace_root_for_filter = workspace_root.clone();
                    let source_root_for_filter = source_root.clone();
                    let exclude_patterns = exclude_patterns.clone();
                    let language = group.language.clone();
                    builder.filter_entry(move |entry| {
                        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                        should_include_discovered_path(
                            entry.path(),
                            is_dir,
                            &workspace_root_for_filter,
                            &source_root_for_filter,
                            filter_by_language,
                            &language,
                            &exclude_patterns,
                        )
                    });
                    for entry in builder.build() {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(error) => {
                                record_walk_error(&mut issues, &full_path, &error);
                                continue;
                            }
                        };
                        if let Some(error) = entry.error() {
                            record_walk_error(&mut issues, entry.path(), error);
                        }
                        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                            continue;
                        }
                        if !push_discovered_file_within_limit(
                            &mut all_files,
                            &mut seen,
                            entry.into_path(),
                            &workspace_root,
                            max_files,
                        ) {
                            all_files.sort();
                            return Ok(WorkspaceFileInventory {
                                files: all_files,
                                outcome: WorkspaceInventoryOutcome::Bounded,
                                issues,
                            });
                        }
                    }
                }
            }
        }

        all_files.sort();
        let outcome = if issues.is_empty() {
            WorkspaceInventoryOutcome::Complete
        } else if inspected_source_roots == 0 {
            WorkspaceInventoryOutcome::Unreadable
        } else {
            WorkspaceInventoryOutcome::Partial
        };
        Ok(WorkspaceFileInventory {
            files: all_files,
            outcome,
            issues,
        })
    }

    /// Compare current discovery with stored inventory and return an
    /// incremental refresh plan.
    pub fn build_refresh_plan(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
    ) -> Result<RefreshPlan> {
        Ok(self.build_refresh_outcome(manifest, inputs)?.plan)
    }

    /// Compare discovery with stored state and preserve inventory completeness.
    pub fn build_refresh_outcome(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
    ) -> Result<WorkspaceRefreshOutcome> {
        self.build_refresh_outcome_inner(manifest, inputs, None)
    }

    /// Compare current discovery with stored inventory unless discovery exceeds
    /// `max_current_files`.
    pub fn build_refresh_plan_bounded(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        max_current_files: usize,
    ) -> Result<Option<RefreshPlan>> {
        let outcome =
            self.build_refresh_outcome_inner(manifest, inputs, Some(max_current_files))?;
        if outcome.inventory_outcome == WorkspaceInventoryOutcome::Bounded {
            Ok(None)
        } else {
            Ok(Some(outcome.plan))
        }
    }

    fn build_refresh_outcome_inner(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        max_current_files: Option<usize>,
    ) -> Result<WorkspaceRefreshOutcome> {
        let inventory = self.source_inventory_inner(manifest, max_current_files)?;
        let current_files = inventory.files;
        let workspace_root = manifest.root_dir();
        let stored_map = inputs.inventory_map();
        let normalized_stored_map = stored_map
            .into_values()
            .map(|file| (normalized_compare_key(&workspace_root, &file.path), file))
            .collect::<HashMap<_, _>>();

        let mut files_to_index = Vec::new();
        let mut files_to_remove = Vec::new();
        let mut existing_file_ids = HashMap::new();
        let mut current_file_keys = HashSet::with_capacity(current_files.len());

        for path in current_files {
            let normalized_key = normalized_compare_key(&workspace_root, &path);
            current_file_keys.insert(normalized_key.clone());
            let needs_index = match normalized_stored_map.get(&normalized_key) {
                Some(file) => {
                    existing_file_ids.insert(path.clone(), file.id);
                    stored_file_needs_index(&path, file)
                }
                None => true,
            };

            if needs_index {
                files_to_index.push(path);
            }
        }

        if inventory.outcome.is_complete() {
            for (normalized_key, stored) in normalized_stored_map {
                if !current_file_keys.contains(&normalized_key) {
                    files_to_remove.push(stored.id);
                }
            }
        }
        files_to_remove.sort_unstable();
        files_to_remove.dedup();

        Ok(WorkspaceRefreshOutcome {
            plan: RefreshPlan {
                mode: RefreshMode::Incremental,
                files_to_index,
                files_to_remove,
                existing_file_ids,
            },
            inventory_outcome: inventory.outcome,
            inventory_issues: inventory.issues,
        })
    }
}

fn record_walk_error(
    issues: &mut Vec<WorkspaceInventoryIssue>,
    source_root: &Path,
    error: &ignore::Error,
) {
    issues.push(WorkspaceInventoryIssue {
        path: source_root.to_path_buf(),
        message: error.to_string(),
    });
}

fn inventory_failure_message(inventory: &WorkspaceFileInventory) -> String {
    let detail = inventory
        .issues
        .first()
        .map(|issue| format!("{}: {}", issue.path.display(), issue.message))
        .unwrap_or_else(|| "candidate limit exceeded".to_string());
    format!(
        "workspace inventory is {:?}; deletion safety cannot be proven ({detail})",
        inventory.outcome
    )
}

fn source_file_limit_exceeded(files: &[PathBuf], max_files: Option<usize>) -> bool {
    max_files.is_some_and(|max_files| files.len() > max_files)
}

fn push_discovered_file_within_limit(
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<String>,
    path: PathBuf,
    workspace_root: &Path,
    max_files: Option<usize>,
) -> bool {
    push_discovered_file(files, seen, path, workspace_root);
    !source_file_limit_exceeded(files, max_files)
}

fn modification_time_millis(path: &Path) -> Result<i64> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH)?;
    Ok(duration.as_millis().min(i64::MAX as u128) as i64)
}

fn stored_file_needs_index(path: &Path, file: &StoredFileState) -> bool {
    if !file.indexed || file.retry_required {
        return true;
    }
    let Ok(mtime) = modification_time_millis(path) else {
        return true;
    };
    if mtime != file.modification_time {
        return true;
    }
    let Some(expected_hash) = file.content_hash.as_deref() else {
        return false;
    };
    match current_content_hash(path) {
        Ok(actual_hash) => actual_hash != expected_hash,
        Err(_) => true,
    }
}

fn current_content_hash(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let before = file.metadata()?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        bail!("source metadata changed while hashing {}", path.display());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn workspace_root(manifest: &WorkspaceManifest) -> PathBuf {
    manifest
        .root_dir()
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexical_path(&manifest.root_dir()))
}

fn canonicalize_with_missing_tail(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(mut canonical) = existing.canonicalize() {
            for component in missing.iter().rev() {
                canonical.push(component);
            }
            return normalize_lexical_path(&canonical);
        }
        let Some(name) = existing.file_name() else {
            return normalize_lexical_path(path);
        };
        missing.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            return normalize_lexical_path(path);
        };
        existing = parent;
    }
}

fn resolve_manifest_source_path(
    manifest: &WorkspaceManifest,
    source_path: &Path,
) -> Result<PathBuf> {
    let root = manifest.root_dir();
    if manifest.trusted_source_paths.get() {
        return Ok(normalize_lexical_path(&if source_path.is_absolute() {
            source_path.to_path_buf()
        } else {
            root.join(source_path)
        }));
    }

    if source_path.is_absolute() {
        bail!(
            "repo-local manifest source path `{}` must be relative to `{}`",
            source_path.display(),
            root.display()
        );
    }

    let full_path = normalize_lexical_path(&root.join(source_path));
    let canonical_root = workspace_root(manifest);
    let canonical_source = canonicalize_with_missing_tail(&full_path);
    if relative_path_for_matching(&canonical_source, &canonical_root).is_none() {
        bail!(
            "repo-local manifest source path `{}` resolves outside manifest root `{}`",
            source_path.display(),
            root.display()
        );
    }

    Ok(full_path)
}

fn compile_exclude_patterns(patterns: &[String]) -> Result<Vec<CompiledExcludePattern>> {
    patterns
        .iter()
        .map(|pattern| {
            let mut patterns = vec![glob::Pattern::new(pattern).map_err(anyhow::Error::from)?];
            if let Some(root_relative) = pattern.strip_prefix("**/") {
                patterns.push(glob::Pattern::new(root_relative).map_err(anyhow::Error::from)?);
            }
            Ok(CompiledExcludePattern {
                raw: pattern.clone(),
                patterns,
                match_absolute: Path::new(pattern).is_absolute(),
            })
        })
        .collect()
}

fn discovery_root(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| normalize_lexical_path(path))
}

fn should_include_discovered_path(
    path: &Path,
    is_dir: bool,
    workspace_root: &Path,
    source_root: &Path,
    filter_by_language: bool,
    language: &Language,
    exclude_patterns: &[CompiledExcludePattern],
) -> bool {
    let normalized = normalize_lexical_path(path);
    if let Ok(canonical) = normalized.canonicalize()
        && !canonical.starts_with(source_root)
    {
        return false;
    }
    if is_excluded_path(&normalized, workspace_root, source_root, exclude_patterns) {
        return false;
    }
    if is_dir {
        return true;
    }
    !filter_by_language || matches_source_group_language(&normalized, language)
}

fn is_excluded_path(
    path: &Path,
    workspace_root: &Path,
    source_root: &Path,
    exclude_patterns: &[CompiledExcludePattern],
) -> bool {
    exclude_patterns.iter().any(|pattern| {
        (pattern.match_absolute && pattern.matches(path))
            || relative_path_for_matching(path, workspace_root)
                .as_deref()
                .is_some_and(|relative| pattern.matches(relative))
            || relative_path_for_matching(path, source_root)
                .as_deref()
                .is_some_and(|relative| pattern.matches(relative))
    })
}

impl CompiledExcludePattern {
    fn matches(&self, path: &Path) -> bool {
        self.patterns
            .iter()
            .any(|pattern| pattern.matches_path(path))
            || self.matches_root_or_nested_directory(path)
    }

    fn matches_root_or_nested_directory(&self, path: &Path) -> bool {
        let raw = self.raw.replace('\\', "/");
        let Some(directory) = raw
            .strip_prefix("**/")
            .and_then(|value| value.strip_suffix("/**"))
        else {
            return false;
        };
        let path = path.to_string_lossy().replace('\\', "/");
        path == directory
            || path.starts_with(&format!("{directory}/"))
            || path.contains(&format!("/{directory}/"))
    }
}

fn relative_path_for_matching(path: &Path, root: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_path_buf());
    }

    if let (Ok(canonical_path), Ok(canonical_root)) = (path.canonicalize(), root.canonicalize())
        && let Ok(relative) = canonical_path.strip_prefix(canonical_root)
    {
        return Some(relative.to_path_buf());
    }

    let path_key = normalize_exclude_match_key(path);
    let root_key = normalize_exclude_match_key(root);
    if path_key == root_key {
        return Some(PathBuf::new());
    }
    let root_prefix = format!("{}/", root_key.trim_end_matches('/'));
    path_key
        .strip_prefix(&root_prefix)
        .map(|relative| PathBuf::from(relative.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

fn normalize_exclude_match_key(path: &Path) -> String {
    normalize_path_key(path)
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_string()
}

fn matches_source_group_language(path: &Path, language: &Language) -> bool {
    if matches!(language, Language::Rust)
        && codestory_contracts::language_support::is_cargo_manifest_file_path(
            path.to_string_lossy().as_ref(),
        )
    {
        return true;
    }

    let Some(extension) = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(codestory_contracts::language_support::normalize_extension)
    else {
        return false;
    };

    registry_extension_matches_source_group(&extension, language)
        || compatibility_extension_matches_source_group(&extension, language)
}

fn registry_extension_matches_source_group(extension: &str, language: &Language) -> bool {
    codestory_contracts::language_support::language_support_profile_for_ext(extension).is_some_and(
        |profile| source_group_accepts_registry_language(language, profile.language_name),
    )
}

fn source_group_accepts_registry_language(language: &Language, registry_language: &str) -> bool {
    matches!(
        (language, registry_language),
        (&Language::Rust, "rust")
            | (&Language::Python, "python")
            | (&Language::Java, "java")
            | (&Language::JavaScript, "javascript")
            | (&Language::TypeScript, "typescript")
            | (&Language::Cxx, "cpp" | "c")
            | (&Language::Go, "go")
            | (&Language::Ruby, "ruby")
            | (&Language::Php, "php")
            | (&Language::CSharp, "csharp")
            | (&Language::Kotlin, "kotlin")
            | (&Language::Swift, "swift")
            | (&Language::Dart, "dart")
            | (&Language::Sql, "sql")
            | (&Language::Html, "html")
            | (&Language::Css, "css")
            | (&Language::Bash, "bash")
    )
}

fn compatibility_extension_matches_source_group(extension: &str, language: &Language) -> bool {
    matches!(
        (language, extension),
        (&Language::JavaScript, "svelte" | "vue" | "astro")
            | (&Language::TypeScript, "svelte" | "vue" | "astro")
            | (&Language::CSharp, "cshtml")
            | (&Language::Lua, "lua")
            | (&Language::Css, "scss" | "sass" | "less")
            | (&Language::PowerShell, "ps1" | "psm1")
            | (&Language::Svelte, "svelte")
            | (&Language::Vue, "vue")
            | (&Language::Astro, "astro")
    )
}

#[cfg(test)]
fn registry_language_for_path(path: &Path) -> Option<&'static str> {
    path.to_str()
        .and_then(|path| codestory_contracts::language_support::language_name_for_path(Some(path)))
}

fn push_discovered_file(
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<String>,
    path: PathBuf,
    workspace_root: &Path,
) {
    let normalized = normalize_lexical_path(&path);
    let key = normalized_compare_key(workspace_root, &normalized);
    if seen.insert(key) {
        files.push(normalized);
    }
}

fn normalized_compare_key(root: &Path, path: &Path) -> String {
    let absolute = if path.is_absolute() {
        normalize_lexical_path(path)
    } else {
        normalize_lexical_path(&root.join(path))
    };
    let stable = absolute.canonicalize().unwrap_or(absolute);
    normalize_path_key(&stable)
}

/// Return `path` relative to `workspace_root` using native path identity.
///
/// Exact lexical prefixes are preferred. Alias spellings then compare each
/// candidate ancestor with the workspace root using filesystem identity for
/// existing paths and platform lexical rules only for missing paths.
pub fn workspace_relative_path(workspace_root: &Path, path: &Path) -> Option<PathBuf> {
    let root = normalize_lexical_path(workspace_root);
    let candidate = normalize_lexical_path(path);
    if let Ok(relative) = candidate.strip_prefix(&root) {
        return Some(relative.to_path_buf());
    }
    if !candidate.is_absolute() {
        return Some(candidate);
    }

    let matching_root = candidate
        .ancestors()
        .find(|ancestor| same_workspace_path(&root, ancestor))?;
    candidate
        .strip_prefix(matching_root)
        .ok()
        .map(Path::to_path_buf)
}

fn normalize_path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io;
    use std::path::Path;
    use tempfile::tempdir;

    fn write_repo_manifest(root: &Path, source_paths: Vec<PathBuf>) -> Result<()> {
        let settings = WorkspaceSettings {
            name: "repo".to_string(),
            version: 1,
            source_groups: vec![SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Rust,
                standard: LanguageStandard::Default,
                source_paths,
                exclude_patterns: Vec::new(),
                include_paths: Vec::new(),
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            }],
        };
        fs::write(
            root.join("codestory_project.json"),
            serde_json::to_string_pretty(&settings)?,
        )?;
        Ok(())
    }

    #[test]
    fn builds_incremental_refresh_plan_without_storage_dependency() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time: 0,
                    content_hash: None,
                    indexed: true,
                    complete: true,
                    retry_required: false,
                }],
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert_eq!(plan.mode, RefreshMode::Incremental);
        assert_eq!(plan.files_to_index, vec![file]);
        assert!(plan.files_to_remove.is_empty());
        Ok(())
    }

    #[test]
    fn incremental_refresh_uses_millisecond_precision_for_unchanged_files() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time: modification_time_millis(&file)?,
                    content_hash: Some(current_content_hash(&file)?),
                    indexed: true,
                    complete: true,
                    retry_required: false,
                }],
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert!(
            plan.files_to_index.is_empty(),
            "unchanged files should not look dirty when stored mtimes use file-table millisecond precision"
        );
        assert_eq!(plan.existing_file_ids.get(&file), Some(&7));
        Ok(())
    }

    #[test]
    fn incremental_refresh_detects_changed_content_with_matching_mtime() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn first() {}\n")?;
        let indexed_hash = current_content_hash(&file)?;
        fs::write(&file, "fn other() {}\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time: modification_time_millis(&file)?,
                    content_hash: Some(indexed_hash),
                    indexed: true,
                    complete: true,
                    retry_required: false,
                }],
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert_eq!(plan.files_to_index, vec![file]);
        Ok(())
    }

    #[test]
    fn incremental_refresh_does_not_spin_on_unchanged_parser_partial_files() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let modification_time = modification_time_millis(&file)?;
        let manifest = WorkspaceManifest::open(root)?;

        let inputs = [
            RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time,
                    content_hash: None,
                    indexed: true,
                    complete: false,
                    retry_required: false,
                }],
                inventory: WorkspaceInventory::default(),
            },
            RefreshInputs {
                stored_files: Vec::new(),
                inventory: WorkspaceInventory::from_records([(
                    file.clone(),
                    IndexedFileRecord {
                        file_id: 7,
                        modification_time,
                        content_hash: None,
                        indexed: true,
                        complete: false,
                        retry_required: false,
                    },
                )]),
            },
        ];

        for input in inputs {
            let plan = WorkspaceDiscovery.build_refresh_plan(&manifest, &input)?;
            assert!(
                plan.files_to_index.is_empty(),
                "parser coverage is diagnostic state, not source freshness"
            );
            assert_eq!(plan.existing_file_ids.get(&file), Some(&7));
        }
        Ok(())
    }

    #[test]
    fn incremental_refresh_retries_unchanged_file_level_failures() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let modification_time = modification_time_millis(&file)?;
        let manifest = WorkspaceManifest::open(root)?;
        let inputs = [
            RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time,
                    content_hash: None,
                    indexed: true,
                    complete: false,
                    retry_required: true,
                }],
                inventory: WorkspaceInventory::default(),
            },
            RefreshInputs {
                stored_files: Vec::new(),
                inventory: WorkspaceInventory::from_records([(
                    file.clone(),
                    IndexedFileRecord {
                        file_id: 7,
                        modification_time,
                        content_hash: None,
                        indexed: true,
                        complete: false,
                        retry_required: true,
                    },
                )]),
            },
        ];

        for input in inputs {
            let plan = WorkspaceDiscovery.build_refresh_plan(&manifest, &input)?;
            assert_eq!(plan.files_to_index, vec![file.clone()]);
            assert_eq!(plan.existing_file_ids.get(&file), Some(&7));
        }
        Ok(())
    }

    #[test]
    fn incremental_refresh_normalizes_paths_and_removes_deleted_inventory_entries() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("src"))?;
        let file = root.join("src").join("..").join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let deleted = root.join("deleted.rs");

        let manifest = WorkspaceManifest::open(root.clone())?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: Vec::new(),
                inventory: WorkspaceInventory::from_records([
                    (
                        file.clone(),
                        IndexedFileRecord {
                            file_id: 11,
                            modification_time: modification_time_millis(&file)?,
                            content_hash: None,
                            indexed: true,
                            complete: true,
                            retry_required: false,
                        },
                    ),
                    (
                        deleted.clone(),
                        IndexedFileRecord {
                            file_id: 19,
                            modification_time: 0,
                            content_hash: None,
                            indexed: true,
                            complete: true,
                            retry_required: false,
                        },
                    ),
                ]),
            },
        )?;

        let normalized_main = root.join("main.rs");
        assert!(
            plan.files_to_index.is_empty(),
            "path-normalized inventory matches should not force reindex"
        );
        assert_eq!(plan.existing_file_ids.get(&normalized_main), Some(&11));
        assert_eq!(plan.files_to_remove, vec![19]);
        Ok(())
    }

    #[test]
    fn partial_inventory_indexes_observed_files_without_deleting_stored_files() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        let observed = root.join("src").join("lib.rs");
        fs::write(&observed, "pub fn observed() {}\n")?;
        write_repo_manifest(
            &root,
            vec![PathBuf::from("src"), PathBuf::from("unreadable")],
        )?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let outcome = manifest.build_execution_outcome(&RefreshInputs {
            stored_files: vec![StoredFileState {
                id: 19,
                path: root.join("still-present.rs"),
                modification_time: 0,
                content_hash: None,
                indexed: true,
                complete: true,
                retry_required: false,
            }],
            inventory: WorkspaceInventory::default(),
        })?;

        assert_eq!(
            outcome.inventory_outcome,
            WorkspaceInventoryOutcome::Partial
        );
        assert_eq!(outcome.plan.files_to_index, vec![observed]);
        assert!(outcome.plan.files_to_remove.is_empty());
        assert_eq!(outcome.inventory_issues.len(), 1);
        Ok(())
    }

    #[test]
    fn unavailable_source_root_is_unreadable_and_never_deletes() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        write_repo_manifest(&root, vec![PathBuf::from("unreadable")])?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let outcome = manifest.build_execution_outcome(&RefreshInputs {
            stored_files: vec![StoredFileState {
                id: 19,
                path: root.join("stored.rs"),
                modification_time: 0,
                content_hash: None,
                indexed: true,
                complete: true,
                retry_required: false,
            }],
            inventory: WorkspaceInventory::default(),
        })?;

        assert_eq!(
            outcome.inventory_outcome,
            WorkspaceInventoryOutcome::Unreadable
        );
        assert!(outcome.plan.files_to_index.is_empty());
        assert!(outcome.plan.files_to_remove.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_source_root_is_unreadable_and_never_deletes() -> Result<()> {
        use std::os::unix::net::UnixListener;

        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let socket_path = root.join("source.sock");
        let _listener = UnixListener::bind(&socket_path)?;
        write_repo_manifest(&root, vec![PathBuf::from("source.sock")])?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let outcome = manifest.build_execution_outcome(&RefreshInputs {
            stored_files: vec![StoredFileState {
                id: 19,
                path: root.join("stored.rs"),
                modification_time: 0,
                content_hash: None,
                indexed: true,
                complete: true,
                retry_required: false,
            }],
            inventory: WorkspaceInventory::default(),
        })?;

        assert_eq!(
            outcome.inventory_outcome,
            WorkspaceInventoryOutcome::Unreadable
        );
        assert!(outcome.plan.files_to_index.is_empty());
        assert!(outcome.plan.files_to_remove.is_empty());
        assert_eq!(outcome.inventory_issues.len(), 1);
        Ok(())
    }

    #[test]
    fn completed_empty_root_plus_unavailable_root_is_partial() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("empty"))?;
        write_repo_manifest(
            &root,
            vec![PathBuf::from("empty"), PathBuf::from("unreadable")],
        )?;

        let manifest = WorkspaceManifest::open(root)?;
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Partial);
        assert!(inventory.files.is_empty());
        assert_eq!(inventory.issues.len(), 1);
        Ok(())
    }

    #[test]
    fn bounded_inventory_never_deletes() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        fs::write(root.join("one.rs"), "pub fn one() {}\n")?;
        fs::write(root.join("two.rs"), "pub fn two() {}\n")?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let outcome = manifest.build_execution_outcome_bounded(
            &RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 19,
                    path: root.join("stored.rs"),
                    modification_time: 0,
                    content_hash: None,
                    indexed: true,
                    complete: true,
                    retry_required: false,
                }],
                inventory: WorkspaceInventory::default(),
            },
            1,
        )?;

        assert_eq!(
            outcome.inventory_outcome,
            WorkspaceInventoryOutcome::Bounded
        );
        assert!(outcome.plan.files_to_remove.is_empty());
        assert!(outcome.plan.files_to_index.len() > 1);
        Ok(())
    }

    #[test]
    fn ignore_processing_errors_make_the_real_walk_partial() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        fs::write(root.join("lib.rs"), "pub fn indexed() {}\n")?;
        fs::write(root.join(".ignore"), "{a,b\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Partial);
        assert!(inventory.files.iter().any(|path| path.ends_with("lib.rs")));
        assert!(!inventory.issues.is_empty());
        Ok(())
    }

    #[test]
    fn source_files_apply_language_filter_and_prune_ignored_directories() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("target"))?;
        fs::create_dir_all(root.join("node_modules"))?;
        fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
        fs::write(root.join("app.tsx"), "export const App = () => <div />;\n")?;
        fs::write(
            root.join("target").join("generated.rs"),
            "pub fn ignored() {}\n",
        )?;
        fs::write(
            root.join("node_modules").join("dep.rs"),
            "pub fn also_ignored() {}\n",
        )?;

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::Rust,
                    standard: LanguageStandard::Default,
                    source_paths: vec![root.clone()],
                    exclude_patterns: vec![
                        "**/target/**".to_string(),
                        "**/node_modules/**".to_string(),
                    ],
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert_eq!(files, vec![root.join("src").join("lib.rs")]);
        Ok(())
    }

    #[test]
    fn source_files_keep_non_rust_files_when_manifest_is_synthetic() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("app.ts"), "export const app = true;\n")?;
        fs::write(
            root.join("App.svelte"),
            "<script>export let app;</script>\n",
        )?;
        fs::write(root.join("src").join("main.py"), "print('hello')\n")?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let files = WorkspaceDiscovery.source_files(&manifest)?;

        assert!(files.contains(&root.join("app.ts")));
        assert!(files.contains(&root.join("App.svelte")));
        assert!(files.contains(&root.join("src").join("main.py")));
        Ok(())
    }

    #[test]
    fn workspace_supported_source_extensions_have_registry_profiles() {
        let source_group_languages = [
            Language::Rust,
            Language::Python,
            Language::Java,
            Language::JavaScript,
            Language::TypeScript,
            Language::Cxx,
            Language::Go,
            Language::Ruby,
            Language::Php,
            Language::CSharp,
            Language::Kotlin,
            Language::Swift,
            Language::Dart,
            Language::Sql,
            Language::Html,
            Language::Css,
            Language::Bash,
        ];

        for profile in codestory_contracts::language_support::LANGUAGE_SUPPORT_PROFILES {
            for extension in profile.extensions {
                let file_name = format!("main.{extension}");
                assert_eq!(
                    registry_language_for_path(Path::new(&file_name)),
                    Some(profile.language_name),
                    "workspace source extension should resolve registry language: {extension}"
                );
                assert!(
                    source_group_languages
                        .iter()
                        .any(|language| matches_source_group_language(
                            Path::new(&file_name),
                            language
                        )),
                    "workspace discovery should accept public registry extension: {extension}"
                );
            }
        }

        let compatibility_only = [
            ("cshtml", Language::CSharp),
            ("svelte", Language::JavaScript),
            ("svelte", Language::TypeScript),
            ("svelte", Language::Svelte),
            ("vue", Language::JavaScript),
            ("vue", Language::TypeScript),
            ("vue", Language::Vue),
            ("astro", Language::JavaScript),
            ("astro", Language::TypeScript),
            ("astro", Language::Astro),
            ("lua", Language::Lua),
            ("ps1", Language::PowerShell),
            ("psm1", Language::PowerShell),
            ("scss", Language::Css),
            ("sass", Language::Css),
            ("less", Language::Css),
        ];
        for (extension, language) in compatibility_only {
            assert!(
                codestory_contracts::language_support::language_support_profile_for_ext(extension)
                    .is_none(),
                "compatibility-only source extension should not have a public registry profile: {extension}"
            );
            let file_name = format!("main.{extension}");
            assert!(
                matches_source_group_language(Path::new(&file_name), &language),
                "compatibility-only source extension should stay accepted by workspace discovery: {extension}"
            );
        }
    }

    #[test]
    fn rust_source_groups_keep_cargo_manifest_but_not_generic_toml() {
        assert!(matches_source_group_language(
            Path::new("Cargo.toml"),
            &Language::Rust
        ));
        assert!(matches_source_group_language(
            Path::new("crates/tool/Cargo.toml"),
            &Language::Rust
        ));
        assert!(!matches_source_group_language(
            Path::new("config.toml"),
            &Language::Rust
        ));
        assert!(!matches_source_group_language(
            Path::new("Cargo.lock"),
            &Language::Rust
        ));
    }

    #[test]
    fn synthetic_workspace_under_excluded_parent_still_discovers_repo_files() -> Result<()> {
        let temp = tempdir()?;
        let root = temp
            .path()
            .join("target")
            .join("agent-benchmark")
            .join("repos")
            .join("express");
        fs::create_dir_all(root.join("lib"))?;
        fs::create_dir_all(root.join("target"))?;
        fs::write(
            root.join("lib").join("application.js"),
            "exports.init = function init() {};\n",
        )?;
        fs::write(
            root.join("target").join("generated.js"),
            "exports.generated = true;\n",
        )?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let files = WorkspaceDiscovery.source_files(&manifest)?;

        assert!(
            files.contains(&root.join("lib").join("application.js")),
            "parent target directory should not exclude the selected workspace: {files:?}"
        );
        assert!(
            !files.contains(&root.join("target").join("generated.js")),
            "repo-local target directory should remain excluded: {files:?}"
        );
        Ok(())
    }

    #[test]
    fn source_files_keep_svelte_in_typescript_source_groups_as_text_evidence() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src").join("App.svelte"), "<script></script>\n")?;
        fs::write(
            root.join("src").join("App.ts"),
            "export const app = true;\n",
        )?;

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::TypeScript,
                    standard: LanguageStandard::Default,
                    source_paths: vec![root.join("src")],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert!(files.contains(&root.join("src").join("App.ts")));
        assert!(files.contains(&root.join("src").join("App.svelte")));
        Ok(())
    }

    #[test]
    fn workspace_manifest_discovers_all_member_roots() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("backend").join("src"))?;
        fs::create_dir_all(root.join("frontend"))?;
        fs::create_dir_all(root.join("backend").join("target"))?;
        fs::write(
            root.join("codestory_workspace.json"),
            r#"{"members":["backend","frontend"]}"#,
        )?;
        fs::write(
            root.join("backend").join("src").join("lib.rs"),
            "pub fn api() {}\n",
        )?;
        fs::write(
            root.join("backend").join("target").join("generated.rs"),
            "pub fn ignored() {}\n",
        )?;
        fs::write(
            root.join("frontend").join("app.ts"),
            "export const app = true;\n",
        )?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let files = WorkspaceDiscovery.source_files(&manifest)?;

        assert_eq!(
            manifest.members(),
            &[PathBuf::from("backend"), PathBuf::from("frontend")]
        );
        assert!(files.contains(&root.join("backend").join("src").join("lib.rs")));
        assert!(files.contains(&root.join("frontend").join("app.ts")));
        assert!(!files.contains(&root.join("backend").join("target").join("generated.rs")));
        Ok(())
    }

    #[test]
    fn repo_local_manifest_rejects_absolute_source_paths_by_default() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src").join("lib.rs"), "pub fn local() {}\n")?;
        write_repo_manifest(&root, vec![root.join("src")])?;

        let manifest = WorkspaceManifest::open(root)?;
        let error = WorkspaceDiscovery
            .source_files(&manifest)
            .expect_err("repo-local manifest should reject absolute source paths");

        assert!(
            error.to_string().contains("must be relative"),
            "absolute source path rejection should explain the trusted boundary: {error}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn repo_local_manifest_allows_missing_paths_beneath_a_symlinked_root() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempdir()?;
        let actual = temp.path().join("actual");
        let alias = temp.path().join("alias");
        fs::create_dir_all(&actual)?;
        symlink(&actual, &alias)?;
        write_repo_manifest(&actual, vec![PathBuf::from("unreadable")])?;

        let manifest = WorkspaceManifest::open(alias.clone())?;
        let resolved = resolve_manifest_source_path(&manifest, Path::new("unreadable"))?;

        assert_eq!(resolved, alias.join("unreadable"));
        Ok(())
    }

    #[test]
    fn repo_local_manifest_rejects_paths_that_escape_manifest_root_by_default() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let shared = temp.path().join("shared");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&shared)?;
        fs::write(shared.join("escape.rs"), "pub fn escape() {}\n")?;
        write_repo_manifest(&root, vec![PathBuf::from("../shared")])?;

        let manifest = WorkspaceManifest::open(root)?;
        let error = WorkspaceDiscovery
            .source_files(&manifest)
            .expect_err("repo-local manifest should reject source paths outside its root");

        assert!(
            error.to_string().contains("outside manifest root"),
            "outside-root rejection should explain the trusted boundary: {error}"
        );
        Ok(())
    }

    #[test]
    fn source_files_allow_explicit_roots_outside_workspace_root() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let shared = temp.path().join("shared");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&shared)?;
        fs::write(shared.join("shared.ts"), "export const shared = true;\n")?;
        fs::write(shared.join("ignored.js"), "export const ignored = true;\n")?;

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::TypeScript,
                    standard: LanguageStandard::Default,
                    source_paths: vec![PathBuf::from("../shared")],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert_eq!(files, vec![shared.join("shared.ts")]);
        Ok(())
    }

    #[test]
    fn source_files_reject_symlinked_directories_outside_workspace_root() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(&outside)?;
        fs::write(root.join("src").join("inside.rs"), "pub fn inside() {}\n")?;
        fs::write(outside.join("escape.rs"), "pub fn escape() {}\n")?;

        let link_path = root.join("linked-outside");
        if let Err(err) = try_create_dir_link(&link_path, &outside) {
            if err.kind() == io::ErrorKind::PermissionDenied {
                return Ok(());
            }
            return Err(err.into());
        }

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::Rust,
                    standard: LanguageStandard::Default,
                    source_paths: vec![root.clone()],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert_eq!(files, vec![root.join("src").join("inside.rs")]);
        Ok(())
    }

    #[cfg(windows)]
    fn try_create_dir_link(link: &Path, target: &Path) -> io::Result<()> {
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.display().to_string(),
                &target.display().to_string(),
            ])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "mklink /J failed with status {status}"
            )))
        }
    }

    #[cfg(not(windows))]
    fn try_create_dir_link(link: &Path, target: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn modification_time_millis(path: &Path) -> Result<i64> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?;
        let duration = modified.duration_since(std::time::UNIX_EPOCH)?;
        Ok(duration.as_millis().min(i64::MAX as u128) as i64)
    }
}
