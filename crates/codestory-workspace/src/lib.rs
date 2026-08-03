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
    BuildMode, DEFAULT_SOURCE_FILE_BYTE_CAP, IndexedFileRecord, OVERSIZED_SOURCE_POLICY_VERSION,
    OversizedSourceExclusionCandidate, RefreshExecutionPlan, RefreshInfo, RefreshInputs,
    RefreshMode, RefreshPlan, SourceIndexPolicy, StoredFileRecord, StoredFileState,
    WorkspaceInventory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub mod atomic_file;
pub mod locking;
pub mod owned_deletion;
pub mod paths;
pub mod source_freshness;
pub use source_freshness::{
    SourceFreshnessCounts, SourceFreshnessScope, record_readiness_fingerprint_pass,
    reverify_from_content as reverify_source_freshness_from_content, source_freshness_counts,
};
mod repo_metadata;
mod repository_hooks;
mod repository_identity;
pub use repo_metadata::{
    RepositoryChange, RepositoryChangeKind, RepositoryChangeScope, RepositoryMetadata,
    RepositoryMetadataIssue, read_repository_changes, read_repository_metadata,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use repo_metadata::{
    with_repository_metadata_observation_limit_for_test,
    with_repository_metadata_tree_traversal_count_for_test,
};
pub use repository_hooks::{
    RepositoryHookAction, RepositoryHookReport, RepositoryHookRequest, RepositoryHookTargetReport,
    manage_repository_hooks,
};
pub use repository_identity::{
    LogicalProjectIdentityV3, PROJECT_IDENTITY_V3_SCHEMA_VERSION, ProjectIdentityV3,
    REPOSITORY_IDENTITY_V2_SCHEMA_VERSION, RepositoryIdentityV2, RepositoryInstanceIdentity,
    WorkspacePathIdentity, WorkspacePathLexicalIdentity, inspect_repository_identity_v2,
    observe_logical_project_identity_v3, project_identity_v3, project_identity_v3_from_repository,
    same_workspace_path, workspace_file_identity, workspace_id_v3_for_root,
    workspace_path_identity, workspace_path_identity_token, workspace_path_lexical_identity,
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
    Shell,
    PowerShell,
    Markdown,
    Yaml,
    Toml,
    Json,
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
    discovery_excluded_files: Vec<PathBuf>,
    discovery_excluded_directory_roots: Vec<PathBuf>,
    discovery_owned_storage_paths: Vec<PathBuf>,
    #[cfg(test)]
    discovery_exclusion_observation_count: Cell<usize>,
}

fn default_source_exclude_patterns() -> Vec<String> {
    [
        "**/node_modules/**",
        "**/target/**",
        "**/.git/**",
        "**/dist/**",
        "**/build/**",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
fn path_with_native_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut suffixed = path.as_os_str().to_os_string();
    suffixed.push(suffix);
    PathBuf::from(suffixed)
}

fn storage_parent_for_observation(storage_path: &Path) -> &Path {
    storage_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn storage_owned_discovery_files(storage_path: &Path) -> Vec<PathBuf> {
    codestory_contracts::owned_artifacts::storage_owned_file_identities(storage_path)
}

/// Owned directory trees discovery must never walk into as user source.
///
/// The quarantine tree belongs here for the same reason the search trees do:
/// the guided derived-cache reset moves owned artifacts into it rather than
/// deleting them, so every one of those files stays on disk inside the cache
/// root under a name discovery has never seen.
fn storage_owned_discovery_directory_roots(storage_path: &Path) -> Vec<PathBuf> {
    vec![
        legacy_search_directory_for_storage(storage_path),
        search_generation_directory_for_storage(storage_path),
        codestory_contracts::owned_artifacts::derived_reset_quarantine_root(storage_path),
    ]
}

fn storage_owned_staged_discovery_files(storage_path: &Path) -> io::Result<Vec<PathBuf>> {
    let parent = storage_parent_for_observation(storage_path);
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut owned = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if codestory_contracts::owned_artifacts::is_staged_snapshot_name(storage_path, &name) {
            owned.push(entry.path());
        }
    }
    owned.sort();
    Ok(owned)
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
///
/// `issues` record inputs discovery could not account for and demote
/// `outcome`. `warnings` record inputs that are present and indexed but whose
/// discovery took a degraded route, so a warned inventory stays distinguishable
/// from a pristine one without refusing service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFileInventory {
    pub files: Vec<PathBuf>,
    pub outcome: WorkspaceInventoryOutcome,
    pub issues: Vec<WorkspaceInventoryIssue>,
    pub warnings: Vec<WorkspaceInventoryIssue>,
}

/// Complete discovery split into parser candidates and verified policy exclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePolicyFileInventory {
    pub files: Vec<PathBuf>,
    pub policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    pub outcome: WorkspaceInventoryOutcome,
    pub issues: Vec<WorkspaceInventoryIssue>,
    pub warnings: Vec<WorkspaceInventoryIssue>,
}

/// Refresh plan paired with the inventory outcome that made deletion safe or unsafe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRefreshOutcome {
    pub plan: RefreshPlan,
    pub inventory_outcome: WorkspaceInventoryOutcome,
    pub inventory_issues: Vec<WorkspaceInventoryIssue>,
    pub inventory_warnings: Vec<WorkspaceInventoryIssue>,
}

/// Refresh plan paired with the complete exclusion set for the same discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePolicyRefreshOutcome {
    pub refresh: WorkspaceRefreshOutcome,
    pub policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
}

#[derive(Debug, Clone)]
struct CompiledExcludePattern {
    raw: String,
    patterns: Vec<glob::Pattern>,
    match_absolute: bool,
}

#[derive(Debug, Clone)]
struct ObservedDiscoveryDirectoryRoot {
    containment_paths: Vec<WorkspacePathLexicalIdentity>,
}

#[derive(Debug, Clone, Default)]
struct ObservedDiscoveryExclusions {
    file_identities: HashSet<WorkspacePathIdentity>,
    file_lexical_paths: HashSet<WorkspacePathLexicalIdentity>,
    directory_roots: Vec<ObservedDiscoveryDirectoryRoot>,
}

struct DiscoveryPathFilter<'a> {
    workspace_root: &'a Path,
    source_root: &'a Path,
    filter_by_language: bool,
    language: &'a Language,
    exclude_patterns: &'a [CompiledExcludePattern],
    discovery_exclusions: &'a ObservedDiscoveryExclusions,
}

#[derive(Debug)]
struct DiscoveryExclusionObservationError {
    path: PathBuf,
    error: std::io::Error,
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
            discovery_excluded_files: Vec::new(),
            discovery_excluded_directory_roots: Vec::new(),
            discovery_owned_storage_paths: Vec::new(),
            #[cfg(test)]
            discovery_exclusion_observation_count: Cell::new(0),
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
            discovery_excluded_files: Vec::new(),
            discovery_excluded_directory_roots: Vec::new(),
            discovery_owned_storage_paths: Vec::new(),
            #[cfg(test)]
            discovery_exclusion_observation_count: Cell::new(0),
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
            discovery_excluded_files: Vec::new(),
            discovery_excluded_directory_roots: Vec::new(),
            discovery_owned_storage_paths: Vec::new(),
            #[cfg(test)]
            discovery_exclusion_observation_count: Cell::new(0),
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
                exclude_patterns: default_source_exclude_patterns(),
                include_paths: Vec::new(),
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            });
            manifest.is_synthetic_default.set(true);
            manifest.trusted_source_paths.set(true);
            Ok(manifest)
        }
    }

    /// Open a workspace while excluding the exact CodeStory core, search, and
    /// quarantined-reset artifacts owned by `storage_path`.
    pub fn open_with_storage_owned_exclusions(
        root_path: PathBuf,
        storage_path: &Path,
    ) -> Result<Self> {
        let mut manifest = Self::open(root_path)?;
        manifest.exclude_discovery_files(storage_owned_discovery_files(storage_path));
        manifest
            .discovery_owned_storage_paths
            .push(storage_path.to_path_buf());
        manifest.exclude_discovery_directory_roots(storage_owned_discovery_directory_roots(
            storage_path,
        ));
        Ok(manifest)
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
                exclude_patterns: default_source_exclude_patterns(),
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

    /// Exclude caller-owned generated files from source discovery.
    ///
    /// These exclusions are runtime-only and are never persisted into a
    /// project manifest.
    pub fn exclude_discovery_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.discovery_excluded_files.extend(paths);
    }

    /// Exclude caller-owned generated directory trees from source discovery.
    ///
    /// These exclusions are runtime-only and are never persisted into a
    /// project manifest.
    pub fn exclude_discovery_directory_roots(&mut self, roots: impl IntoIterator<Item = PathBuf>) {
        self.discovery_excluded_directory_roots.extend(roots);
    }

    #[cfg(test)]
    fn discovery_exclusion_observation_count(&self) -> usize {
        self.discovery_exclusion_observation_count.get()
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

    /// Discover parser candidates and content-verified oversized policy exclusions.
    pub fn source_inventory_with_oversized_policy(&self) -> Result<WorkspacePolicyFileInventory> {
        self.source_inventory_with_policy(&SourceIndexPolicy::default())
    }

    /// Discover candidates using one caller-owned immutable source-index policy.
    pub fn source_inventory_with_policy(
        &self,
        policy: &SourceIndexPolicy,
    ) -> Result<WorkspacePolicyFileInventory> {
        WorkspaceDiscovery.source_inventory_with_policy_inner(
            self,
            policy.byte_cap,
            &policy.policy_version,
            policy.structural_unit_cap,
            None,
        )
    }

    /// Re-read every classified exclusion at the publication fence.
    pub fn revalidate_source_policy_exclusions(
        &self,
        candidates: &[OversizedSourceExclusionCandidate],
        policy: &SourceIndexPolicy,
    ) -> Result<Vec<OversizedSourceExclusionCandidate>> {
        WorkspaceDiscovery.revalidate_source_policy_exclusions(self, candidates, policy)
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

    /// Build a refresh plan that removes verified oversized sources before parser scheduling.
    pub fn build_execution_outcome_with_oversized_policy(
        &self,
        inputs: &RefreshInputs,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        self.build_execution_outcome_with_policy(inputs, &SourceIndexPolicy::default())
    }

    /// Build a refresh outcome using one caller-owned immutable source-index policy.
    pub fn build_execution_outcome_with_policy(
        &self,
        inputs: &RefreshInputs,
        policy: &SourceIndexPolicy,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        WorkspaceDiscovery.build_refresh_outcome_with_index_policy(self, inputs, policy)
    }

    /// Build a policy-aware refresh outcome with the ordinary current-file discovery bound.
    pub fn build_execution_outcome_bounded_with_oversized_policy(
        &self,
        inputs: &RefreshInputs,
        max_current_files: usize,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        self.build_execution_outcome_bounded_with_policy(
            inputs,
            max_current_files,
            &SourceIndexPolicy::default(),
        )
    }

    /// Build a bounded refresh outcome using one caller-owned immutable policy.
    pub fn build_execution_outcome_bounded_with_policy(
        &self,
        inputs: &RefreshInputs,
        max_current_files: usize,
        policy: &SourceIndexPolicy,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        WorkspaceDiscovery.build_refresh_outcome_bounded_with_index_policy(
            self,
            inputs,
            max_current_files,
            policy,
        )
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

/// Legacy mutable search directory associated with one core database.
pub fn legacy_search_directory_for_storage(storage_path: &Path) -> PathBuf {
    codestory_contracts::owned_artifacts::search_directory_for_storage(
        storage_path,
        codestory_contracts::owned_artifacts::SEARCH_DIRECTORY_SUFFIXES[0],
    )
}

/// Immutable search-generation directory associated with one core database.
pub fn search_generation_directory_for_storage(storage_path: &Path) -> PathBuf {
    codestory_contracts::owned_artifacts::search_directory_for_storage(
        storage_path,
        codestory_contracts::owned_artifacts::SEARCH_DIRECTORY_SUFFIXES[1],
    )
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

    /// Classify oversized sources only after complete discovery and stable content hashing.
    pub fn source_inventory_with_policy(
        &self,
        manifest: &WorkspaceManifest,
        byte_cap: u64,
        policy_version: &str,
    ) -> Result<WorkspacePolicyFileInventory> {
        self.source_inventory_with_policy_inner(
            manifest,
            byte_cap,
            policy_version,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            None,
        )
    }

    /// Verify that classified exclusions still name the same stable bytes at publication time.
    pub fn revalidate_source_policy_exclusions(
        &self,
        manifest: &WorkspaceManifest,
        candidates: &[OversizedSourceExclusionCandidate],
        policy: &SourceIndexPolicy,
    ) -> Result<Vec<OversizedSourceExclusionCandidate>> {
        if policy.byte_cap == 0
            || policy.structural_unit_cap == 0
            || policy.policy_version.trim().is_empty()
        {
            bail!("source index policy requires non-zero caps and a non-empty version");
        }
        let root = workspace_root(manifest);
        let mut verified = candidates.to_vec();
        verified.sort_by(|left, right| left.normalized_path.cmp(&right.normalized_path));
        let mut previous_path: Option<&str> = None;
        for candidate in &verified {
            if candidate.policy_version != policy.policy_version
                || candidate.byte_cap != policy.byte_cap
                || candidate.structural_unit_cap != policy.structural_unit_cap
                || previous_path == Some(candidate.normalized_path.as_str())
            {
                bail!(
                    "source policy exclusion `{}` does not match the active policy",
                    candidate.normalized_path
                );
            }
            previous_path = Some(candidate.normalized_path.as_str());
            let path = root.join(&candidate.normalized_path);
            let normalized_path = normalized_policy_path(&root, &path)?;
            if normalized_path != candidate.normalized_path {
                bail!(
                    "source policy exclusion path changed from `{}` to `{normalized_path}`",
                    candidate.normalized_path
                );
            }
            let (content_hash, observed_size) = current_content_identity(&path)?;
            let byte_bound = observed_size > policy.byte_cap && candidate.observed_unit_count == 0;
            let unit_bound = candidate.observed_unit_count > policy.structural_unit_cap
                && observed_size <= policy.byte_cap;
            if content_hash != candidate.content_hash
                || observed_size != candidate.observed_size
                || !(byte_bound || unit_bound)
            {
                bail!(
                    "source policy exclusion `{}` changed before publication",
                    candidate.normalized_path
                );
            }
        }
        Ok(verified)
    }

    fn source_inventory_with_policy_inner(
        &self,
        manifest: &WorkspaceManifest,
        byte_cap: u64,
        policy_version: &str,
        structural_unit_cap: u64,
        max_files: Option<usize>,
    ) -> Result<WorkspacePolicyFileInventory> {
        if byte_cap == 0 || structural_unit_cap == 0 || policy_version.trim().is_empty() {
            bail!("bounded source policy requires non-zero caps and a non-empty version");
        }
        let inventory = self.source_inventory_inner(manifest, max_files)?;
        if !inventory.outcome.is_complete() {
            return Ok(WorkspacePolicyFileInventory {
                files: inventory.files,
                policy_exclusions: Vec::new(),
                outcome: inventory.outcome,
                issues: inventory.issues,
                warnings: inventory.warnings,
            });
        }

        let root = workspace_root(manifest);
        let mut files = Vec::with_capacity(inventory.files.len());
        let mut policy_exclusions = Vec::new();
        let mut issues = inventory.issues;
        let warnings = inventory.warnings;
        for path in inventory.files {
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    issues.push(WorkspaceInventoryIssue {
                        path: path.clone(),
                        message: format!("failed to inspect policy candidate: {error}"),
                    });
                    files.push(path);
                    continue;
                }
            };
            if metadata.len() <= byte_cap {
                files.push(path);
                continue;
            }
            let normalized_path = match normalized_policy_path(&root, &path) {
                Ok(path) => path,
                Err(error) => {
                    issues.push(WorkspaceInventoryIssue {
                        path: path.clone(),
                        message: error.to_string(),
                    });
                    files.push(path);
                    continue;
                }
            };
            let (content_hash, observed_size) = match current_content_identity(&path) {
                Ok(identity) => identity,
                Err(error) => {
                    issues.push(WorkspaceInventoryIssue {
                        path: path.clone(),
                        message: format!("failed to verify oversized policy candidate: {error}"),
                    });
                    files.push(path);
                    continue;
                }
            };
            if observed_size <= byte_cap {
                files.push(path);
                continue;
            }
            policy_exclusions.push(OversizedSourceExclusionCandidate {
                normalized_path,
                content_hash,
                observed_size,
                observed_unit_count: 0,
                policy_version: policy_version.to_string(),
                byte_cap,
                structural_unit_cap,
            });
        }
        policy_exclusions.sort_by(|left, right| left.normalized_path.cmp(&right.normalized_path));
        let outcome = if issues.is_empty() {
            WorkspaceInventoryOutcome::Complete
        } else {
            WorkspaceInventoryOutcome::Partial
        };
        Ok(WorkspacePolicyFileInventory {
            files,
            policy_exclusions,
            outcome,
            issues,
            warnings,
        })
    }

    fn source_inventory_inner(
        &self,
        manifest: &WorkspaceManifest,
        max_files: Option<usize>,
    ) -> Result<WorkspaceFileInventory> {
        let workspace_root = workspace_root(manifest);
        let (repository_tracked_paths, repository_metadata_issues) =
            repository_tracked_paths(&manifest.root_dir());
        let mut all_files = Vec::new();
        let mut seen = HashSet::new();
        let mut issues = repository_metadata_issues;
        let mut warnings: Vec<WorkspaceInventoryIssue> = Vec::new();
        let mut inspected_source_roots = 0usize;
        let discovery_exclusions = match observe_discovery_exclusions(manifest) {
            Ok(exclusions) => exclusions,
            Err(error) => {
                return Ok(WorkspaceFileInventory {
                    files: Vec::new(),
                    outcome: WorkspaceInventoryOutcome::Unreadable,
                    issues: vec![WorkspaceInventoryIssue {
                        path: error.path,
                        message: format!(
                            "failed to observe caller-owned discovery exclusion: {}",
                            error.error
                        ),
                    }],
                    warnings: Vec::new(),
                });
            }
        };

        for group in &manifest.settings.source_groups {
            let exclude_patterns = compile_exclude_patterns(&group.exclude_patterns)?;
            let filter_by_language = manifest.should_filter_source_group_language();
            for source_path in &group.source_paths {
                let full_path = resolve_manifest_source_path(manifest, source_path)?;
                if discovery_exclusions.directory_contains(&full_path) {
                    continue;
                }
                match discovery_exclusions.file_is_excluded(&full_path) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        issues.push(WorkspaceInventoryIssue {
                            path: full_path,
                            message: format!(
                                "failed to observe source identity against caller-owned exclusions: {error}"
                            ),
                        });
                        continue;
                    }
                }
                if workspace_structural_source_exclusion(&workspace_root, &full_path).is_some() {
                    continue;
                }
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
                    let path_filter = DiscoveryPathFilter {
                        workspace_root: &workspace_root,
                        source_root: &source_root,
                        filter_by_language,
                        language: &group.language,
                        exclude_patterns: &exclude_patterns,
                        discovery_exclusions: &discovery_exclusions,
                    };
                    if !should_include_discovered_path(&full_path, false, &path_filter) {
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
                            warnings,
                        });
                    }
                    continue;
                }
                if metadata.is_dir() {
                    let mut builder = ignore::WalkBuilder::new(&full_path);
                    builder.follow_links(true);
                    builder.require_git(false);
                    builder.parents(false);
                    builder.git_global(false);
                    // Hidden source trees such as .github are product input. The
                    // manifest's explicit excludes still prune repository metadata.
                    builder.hidden(false);
                    let workspace_root_for_filter = workspace_root.clone();
                    let source_root_for_filter = source_root.clone();
                    let exclude_patterns_for_filter = exclude_patterns.clone();
                    let filter_discovery_exclusions = discovery_exclusions.clone();
                    let language = group.language.clone();
                    builder.filter_entry(move |entry| {
                        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                        let path_filter = DiscoveryPathFilter {
                            workspace_root: &workspace_root_for_filter,
                            source_root: &source_root_for_filter,
                            filter_by_language,
                            language: &language,
                            exclude_patterns: &exclude_patterns_for_filter,
                            discovery_exclusions: &filter_discovery_exclusions,
                        };
                        should_include_discovered_path(entry.path(), is_dir, &path_filter)
                    });
                    let issue_count_before_walk = issues.len();
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
                        match discovery_exclusions.file_is_excluded(entry.path()) {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(error) => {
                                issues.push(WorkspaceInventoryIssue {
                                    path: entry.path().to_path_buf(),
                                    message: format!(
                                        "failed to observe source identity against caller-owned exclusions: {error}"
                                    ),
                                });
                                continue;
                            }
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
                                warnings,
                            });
                        }
                    }
                    let walk_had_errors = issues.len() != issue_count_before_walk;
                    let path_filter = DiscoveryPathFilter {
                        workspace_root: &workspace_root,
                        source_root: &source_root,
                        filter_by_language,
                        language: &group.language,
                        exclude_patterns: &exclude_patterns,
                        discovery_exclusions: &discovery_exclusions,
                    };
                    for tracked_path in &repository_tracked_paths {
                        if seen.contains(&normalized_compare_key(&workspace_root, tracked_path))
                            || !fs::metadata(tracked_path).is_ok_and(|metadata| metadata.is_file())
                            || !should_include_discovered_path(tracked_path, false, &path_filter)
                        {
                            continue;
                        }
                        match discovery_exclusions.file_is_excluded(tracked_path) {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(error) => {
                                issues.push(WorkspaceInventoryIssue {
                                    path: tracked_path.clone(),
                                    message: format!(
                                        "failed to observe tracked source identity against caller-owned exclusions: {error}"
                                    ),
                                });
                                continue;
                            }
                        }
                        if !push_discovered_file_within_limit(
                            &mut all_files,
                            &mut seen,
                            tracked_path.clone(),
                            &workspace_root,
                            max_files,
                        ) {
                            all_files.sort();
                            return Ok(WorkspaceFileInventory {
                                files: all_files,
                                outcome: WorkspaceInventoryOutcome::Bounded,
                                issues,
                                warnings,
                            });
                        }
                        if !walk_had_errors {
                            // The file is present and indexed; only its
                            // discovery route was degraded. Recording this as a
                            // warning keeps the inventory complete (a tracked
                            // input that could NOT be restored still pushes an
                            // issue and demotes the outcome) while leaving the
                            // degradation visible to every inventory consumer.
                            warnings.push(WorkspaceInventoryIssue {
                                path: tracked_path.clone(),
                                message: "repository ignore rules excluded a tracked source; the repository index restored it"
                                    .to_string(),
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
            warnings,
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

    /// Build one refresh plan and complete exclusion set from the same inventory pass.
    pub fn build_refresh_outcome_with_policy(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        byte_cap: u64,
        policy_version: &str,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        let inventory = self.source_inventory_with_policy(manifest, byte_cap, policy_version)?;
        let refresh = build_refresh_outcome_from_inventory(
            manifest,
            inputs,
            inventory.files,
            inventory.outcome,
            inventory.issues,
            inventory.warnings,
        )?;
        Ok(WorkspacePolicyRefreshOutcome {
            refresh,
            policy_exclusions: inventory.policy_exclusions,
        })
    }

    /// Build one refresh plan using the complete caller-owned byte/unit policy.
    pub fn build_refresh_outcome_with_index_policy(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        policy: &SourceIndexPolicy,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        let mut inventory = self.source_inventory_with_policy_inner(
            manifest,
            policy.byte_cap,
            &policy.policy_version,
            policy.structural_unit_cap,
            None,
        )?;
        self.carry_forward_verified_policy_exclusions(manifest, inputs, policy, &mut inventory);
        let refresh = build_refresh_outcome_from_inventory(
            manifest,
            inputs,
            inventory.files,
            inventory.outcome,
            inventory.issues,
            inventory.warnings,
        )?;
        Ok(WorkspacePolicyRefreshOutcome {
            refresh,
            policy_exclusions: inventory.policy_exclusions,
        })
    }

    /// Build a bounded policy-aware refresh outcome without losing completeness evidence.
    pub fn build_refresh_outcome_bounded_with_policy(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        max_current_files: usize,
        byte_cap: u64,
        policy_version: &str,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        let inventory = self.source_inventory_with_policy_inner(
            manifest,
            byte_cap,
            policy_version,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            Some(max_current_files),
        )?;
        let refresh = build_refresh_outcome_from_inventory(
            manifest,
            inputs,
            inventory.files,
            inventory.outcome,
            inventory.issues,
            inventory.warnings,
        )?;
        Ok(WorkspacePolicyRefreshOutcome {
            refresh,
            policy_exclusions: inventory.policy_exclusions,
        })
    }

    /// Build a bounded refresh using the complete caller-owned byte/unit policy.
    pub fn build_refresh_outcome_bounded_with_index_policy(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        max_current_files: usize,
        policy: &SourceIndexPolicy,
    ) -> Result<WorkspacePolicyRefreshOutcome> {
        let mut inventory = self.source_inventory_with_policy_inner(
            manifest,
            policy.byte_cap,
            &policy.policy_version,
            policy.structural_unit_cap,
            Some(max_current_files),
        )?;
        self.carry_forward_verified_policy_exclusions(manifest, inputs, policy, &mut inventory);
        let refresh = build_refresh_outcome_from_inventory(
            manifest,
            inputs,
            inventory.files,
            inventory.outcome,
            inventory.issues,
            inventory.warnings,
        )?;
        Ok(WorkspacePolicyRefreshOutcome {
            refresh,
            policy_exclusions: inventory.policy_exclusions,
        })
    }

    fn carry_forward_verified_policy_exclusions(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
        policy: &SourceIndexPolicy,
        inventory: &mut WorkspacePolicyFileInventory,
    ) {
        if !inventory.outcome.is_complete() || inputs.policy_exclusions.is_empty() {
            return;
        }

        let root = workspace_root(manifest);
        let mut current_policy_paths = inventory
            .policy_exclusions
            .iter()
            .map(|candidate| candidate.normalized_path.clone())
            .collect::<HashSet<_>>();
        for candidate in &inputs.policy_exclusions {
            if current_policy_paths.contains(&candidate.normalized_path)
                || candidate.observed_unit_count == 0
                || self
                    .revalidate_source_policy_exclusions(
                        manifest,
                        std::slice::from_ref(candidate),
                        policy,
                    )
                    .is_err()
            {
                continue;
            }

            let candidate_key =
                normalized_compare_key(&root, &root.join(&candidate.normalized_path));
            let Some(position) = inventory
                .files
                .iter()
                .position(|path| normalized_compare_key(&root, path) == candidate_key)
            else {
                continue;
            };
            inventory.files.swap_remove(position);
            current_policy_paths.insert(candidate.normalized_path.clone());
            inventory.policy_exclusions.push(candidate.clone());
        }
        inventory.files.sort();
        inventory
            .policy_exclusions
            .sort_by(|left, right| left.normalized_path.cmp(&right.normalized_path));
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
        build_refresh_outcome_from_inventory(
            manifest,
            inputs,
            inventory.files,
            inventory.outcome,
            inventory.issues,
            inventory.warnings,
        )
    }
}

fn build_refresh_outcome_from_inventory(
    manifest: &WorkspaceManifest,
    inputs: &RefreshInputs,
    current_files: Vec<PathBuf>,
    inventory_outcome: WorkspaceInventoryOutcome,
    inventory_issues: Vec<WorkspaceInventoryIssue>,
    inventory_warnings: Vec<WorkspaceInventoryIssue>,
) -> Result<WorkspaceRefreshOutcome> {
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

    if inventory_outcome.is_complete() {
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
        inventory_outcome,
        inventory_issues,
        inventory_warnings,
    })
}

fn normalized_policy_path(workspace_root: &Path, path: &Path) -> Result<String> {
    let relative = workspace_relative_path(workspace_root, path)
        .ok_or_else(|| anyhow::anyhow!("oversized policy candidate escapes the workspace root"))?;
    let relative = relative
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("oversized policy candidate path is not valid UTF-8"))?
        .replace('\\', "/");
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.split('/').any(|component| component == "..")
    {
        bail!("oversized policy candidate has an invalid normalized path");
    }
    Ok(relative)
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

fn repository_tracked_paths(
    repository_root: &Path,
) -> (Vec<PathBuf>, Vec<WorkspaceInventoryIssue>) {
    let dot_git = repository_root.join(".git");
    let dot_git_metadata = match fs::symlink_metadata(&dot_git) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(error) => {
            return (
                Vec::new(),
                vec![WorkspaceInventoryIssue {
                    path: dot_git,
                    message: format!("repository metadata boundary could not be observed: {error}"),
                }],
            );
        }
    };
    if dot_git_metadata.is_dir() {
        let mut has_repository_marker = false;
        for marker in ["HEAD", "config", "index"] {
            match fs::symlink_metadata(dot_git.join(marker)) {
                Ok(_) => has_repository_marker = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return (
                        Vec::new(),
                        vec![WorkspaceInventoryIssue {
                            path: dot_git.join(marker),
                            message: format!(
                                "repository metadata marker could not be observed: {error}"
                            ),
                        }],
                    );
                }
            }
        }
        if !has_repository_marker {
            return (Vec::new(), Vec::new());
        }
    }
    let metadata = read_repository_metadata(repository_root);
    let mut issues = metadata
        .issues
        .into_iter()
        .filter(|issue| {
            matches!(
                issue.code.as_str(),
                "repository_open_failed" | "index_unavailable" | "repository_metadata_changed"
            )
        })
        .map(|issue| WorkspaceInventoryIssue {
            path: issue.path,
            message: format!(
                "repository metadata observation {} was incomplete: {}",
                issue.code, issue.message
            ),
        })
        .collect::<Vec<_>>();
    let mut paths = Vec::new();
    for encoded_path in metadata.tracked_paths {
        let path = match repo_metadata::bytes_to_path(&encoded_path) {
            Ok(path) => path,
            Err(error) => {
                issues.push(WorkspaceInventoryIssue {
                    path: repository_root.to_path_buf(),
                    message: format!("repository index path could not be decoded: {error}"),
                });
                continue;
            }
        };
        if path.is_absolute()
            || !path
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            issues.push(WorkspaceInventoryIssue {
                path: repository_root.to_path_buf(),
                message: "repository index contained a path outside the workspace boundary"
                    .to_string(),
            });
            continue;
        }
        paths.push(repository_root.join(path));
    }
    paths.sort();
    paths.dedup();
    (paths, issues)
}

/// Clamp one filesystem timestamp to CodeStory's signed Unix-millisecond contract.
///
/// Filesystems that can represent pre-epoch timestamps map them to zero. Large
/// future timestamps saturate instead of wrapping the persisted `i64` value.
pub fn clamp_system_time_to_epoch_millis(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// The mtime and length one freshness observation was taken at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ObservedFileMetadata {
    modified_ms: i64,
    len: u64,
}

fn observed_file_metadata(path: &Path) -> Result<ObservedFileMetadata> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    Ok(ObservedFileMetadata {
        modified_ms: clamp_system_time_to_epoch_millis(modified),
        len: metadata.len(),
    })
}

fn stored_file_needs_index(path: &Path, file: &StoredFileState) -> bool {
    if !file.indexed || file.retry_required {
        return true;
    }
    let Ok(observed) = observed_file_metadata(path) else {
        return true;
    };
    if observed.modified_ms != file.modification_time {
        return true;
    }
    let Some(expected_hash) = file.content_hash.as_deref() else {
        return false;
    };
    // The hash below is the verification, never a redundant double-check: an
    // mtime mismatch already short-circuited, so this is the only mechanism
    // that sees same-mtime drift. The memo caches that verdict for one armed
    // operation scope; it never substitutes metadata for content, and any
    // check whose job is to detect drift *since* an earlier derivation calls
    // `source_freshness::reverify_from_content` first, which drops the memo so
    // this hash runs again.
    if let Some(verdict) =
        source_freshness::memoized_verdict(path, observed.modified_ms, observed.len, expected_hash)
    {
        return verdict;
    }
    source_freshness::record_content_hash_read();
    let Ok(identity) = read_content_identity(path) else {
        return true;
    };
    let verdict = identity.content_hash != expected_hash;
    // Only a torn-read-clean observation whose metadata still agrees with the
    // metadata the key was built from may be memoized. Anything that raced a
    // writer is recomputed next time.
    if identity.modified_ms == observed.modified_ms && identity.len == observed.len {
        source_freshness::record_verdict(
            path,
            observed.modified_ms,
            observed.len,
            expected_hash,
            verdict,
        );
    }
    verdict
}

#[cfg(test)]
fn current_content_hash(path: &Path) -> Result<String> {
    current_content_identity(path).map(|(content_hash, _)| content_hash)
}

fn current_content_identity(path: &Path) -> Result<(String, u64)> {
    let identity = read_content_identity(path)?;
    Ok((identity.content_hash, identity.len))
}

/// A content hash plus the metadata it was verified against.
struct ContentIdentity {
    content_hash: String,
    len: u64,
    modified_ms: i64,
}

fn read_content_identity(path: &Path) -> Result<ContentIdentity> {
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
    Ok(ContentIdentity {
        content_hash: format!("{:x}", hasher.finalize()),
        len: before.len(),
        modified_ms: clamp_system_time_to_epoch_millis(before.modified()?),
    })
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

fn observe_discovery_exclusions(
    manifest: &WorkspaceManifest,
) -> std::result::Result<ObservedDiscoveryExclusions, DiscoveryExclusionObservationError> {
    #[cfg(test)]
    manifest.discovery_exclusion_observation_count.set(0);

    let mut observed = ObservedDiscoveryExclusions::default();
    let mut excluded_files = manifest.discovery_excluded_files.clone();
    for storage_path in &manifest.discovery_owned_storage_paths {
        excluded_files.extend(storage_owned_staged_discovery_files(storage_path).map_err(
            |error| DiscoveryExclusionObservationError {
                path: storage_parent_for_observation(storage_path).to_path_buf(),
                error,
            },
        )?);
    }
    for path in &excluded_files {
        #[cfg(test)]
        manifest
            .discovery_exclusion_observation_count
            .set(manifest.discovery_exclusion_observation_count.get() + 1);
        let identity =
            workspace_path_identity(path).map_err(|error| DiscoveryExclusionObservationError {
                path: path.clone(),
                error,
            })?;
        observed.file_identities.insert(identity);
        observed
            .file_lexical_paths
            .insert(workspace_path_lexical_identity(path).map_err(|error| {
                DiscoveryExclusionObservationError {
                    path: path.clone(),
                    error,
                }
            })?);
    }
    for root in &manifest.discovery_excluded_directory_roots {
        #[cfg(test)]
        manifest
            .discovery_exclusion_observation_count
            .set(manifest.discovery_exclusion_observation_count.get() + 1);
        workspace_path_identity(root).map_err(|error| DiscoveryExclusionObservationError {
            path: root.clone(),
            error,
        })?;

        let mut containment_paths =
            vec![workspace_path_lexical_identity(root).map_err(|error| {
                DiscoveryExclusionObservationError {
                    path: root.clone(),
                    error,
                }
            })?];
        match root.canonicalize() {
            Ok(canonical) => {
                let canonical = workspace_path_lexical_identity(&canonical).map_err(|error| {
                    DiscoveryExclusionObservationError {
                        path: root.clone(),
                        error,
                    }
                })?;
                if !containment_paths.contains(&canonical) {
                    containment_paths.push(canonical);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DiscoveryExclusionObservationError {
                    path: root.clone(),
                    error,
                });
            }
        }
        observed
            .directory_roots
            .push(ObservedDiscoveryDirectoryRoot { containment_paths });
    }
    Ok(observed)
}

impl ObservedDiscoveryExclusions {
    fn file_is_excluded(&self, path: &Path) -> std::io::Result<bool> {
        if self.file_identities.is_empty() {
            return Ok(false);
        }
        if self
            .file_lexical_paths
            .contains(&workspace_path_lexical_identity(path)?)
        {
            return Ok(true);
        }
        workspace_path_identity(path).map(|identity| self.file_identities.contains(&identity))
    }

    fn directory_contains(&self, path: &Path) -> bool {
        if self.directory_roots.is_empty() {
            return false;
        }
        let Ok(lexical) = workspace_path_lexical_identity(path) else {
            return true;
        };
        let canonical = path
            .canonicalize()
            .ok()
            .and_then(|path| workspace_path_lexical_identity(&path).ok());
        self.directory_contains_observed(&lexical, canonical.as_ref())
    }

    fn directory_contains_observed(
        &self,
        lexical: &WorkspacePathLexicalIdentity,
        canonical: Option<&WorkspacePathLexicalIdentity>,
    ) -> bool {
        self.directory_roots
            .iter()
            .flat_map(|root| &root.containment_paths)
            .any(|root| {
                lexical.is_within(root) || canonical.is_some_and(|path| path.is_within(root))
            })
    }
}

fn should_include_discovered_path(
    path: &Path,
    is_dir: bool,
    filter: &DiscoveryPathFilter<'_>,
) -> bool {
    let normalized = normalize_lexical_path(path);
    let canonical = normalized.canonicalize().ok();
    let Ok(exclusion_lexical) = workspace_path_lexical_identity(&normalized) else {
        return false;
    };
    let exclusion_canonical = canonical
        .as_deref()
        .and_then(|path| workspace_path_lexical_identity(path).ok());
    if filter
        .discovery_exclusions
        .directory_contains_observed(&exclusion_lexical, exclusion_canonical.as_ref())
    {
        return false;
    }
    if !is_dir
        && workspace_structural_source_exclusion(filter.workspace_root, &normalized).is_some()
    {
        return false;
    }
    if canonical.is_some_and(|canonical| !canonical.starts_with(filter.source_root)) {
        return false;
    }
    if is_excluded_path(
        &normalized,
        filter.workspace_root,
        filter.source_root,
        filter.exclude_patterns,
    ) {
        return false;
    }
    if is_dir {
        return true;
    }
    !filter.filter_by_language || matches_source_group_language(&normalized, filter.language)
}

fn workspace_structural_source_exclusion(
    workspace_root: &Path,
    path: &Path,
) -> Option<&'static str> {
    let relative = workspace_relative_path(workspace_root, path)?;
    let relative = relative.to_str()?.replace('\\', "/");
    if !codestory_contracts::language_support::is_structural_source_path(&relative) {
        return None;
    }
    codestory_contracts::language_support::structural_source_path_exclusion(&relative)
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
            // Companion-extension source groups. These names never come back
            // from `language_support_profile_for_ext`, so they cannot widen
            // `registry_extension_matches_source_group`; they exist so the
            // companion table can name a source group by language name.
            | (&Language::Lua, "lua")
            | (&Language::Svelte, "svelte")
            | (&Language::Vue, "vue")
            | (&Language::Astro, "astro")
            | (&Language::Sql, "sql")
            | (&Language::Html, "html")
            | (&Language::Css, "css")
            | (&Language::Bash, "bash")
            | (&Language::Shell, "shell")
            | (&Language::PowerShell, "powershell")
            | (&Language::Markdown, "markdown")
            | (&Language::Yaml, "yaml")
            | (&Language::Toml, "toml")
            | (&Language::Json, "json")
    )
}

/// Template and style extensions that a registered language's source group
/// accepts without being a public claim of their own.
///
/// The (language, extension) cross product used to be spelled out here and
/// repeated in the indexer and the CLI (ARCH-012); the extension table now
/// lives in `codestory_contracts::language_support`.
fn compatibility_extension_matches_source_group(extension: &str, language: &Language) -> bool {
    codestory_contracts::language_support::companion_extension_profile(extension).is_some_and(
        |profile| {
            profile
                .source_group_languages
                .iter()
                .any(|group| source_group_accepts_registry_language(language, group))
        },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRelativePathResolution {
    Existing {
        absolute: PathBuf,
        relative: PathBuf,
    },
    Missing {
        absolute: PathBuf,
        relative: PathBuf,
    },
    NotFile {
        absolute: PathBuf,
        relative: PathBuf,
    },
    OutOfProject,
}

/// Resolve an exact path against one project using the workspace's native
/// containment rules.
///
/// Existing paths are checked after filesystem resolution so a symlink cannot
/// turn an apparently project-relative spelling into an outside target.
/// Missing paths use the operation-scoped native lexical identity contract.
pub fn resolve_project_relative_path(
    workspace_root: &Path,
    requested: &Path,
) -> io::Result<ProjectRelativePathResolution> {
    if requested.as_os_str().is_empty() {
        return Ok(ProjectRelativePathResolution::OutOfProject);
    }
    let root = normalize_lexical_path(workspace_root);
    let candidate = normalize_lexical_path(&if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    });
    let Some(relative) = workspace_relative_path(&root, &candidate) else {
        return Ok(ProjectRelativePathResolution::OutOfProject);
    };
    if relative.as_os_str().is_empty() {
        return Ok(ProjectRelativePathResolution::NotFile {
            absolute: candidate,
            relative,
        });
    }

    match fs::metadata(&candidate) {
        Ok(metadata) => {
            let resolved = candidate.canonicalize()?;
            let resolved_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            if workspace_relative_path(&resolved_root, &resolved).is_none() {
                return Ok(ProjectRelativePathResolution::OutOfProject);
            }
            if metadata.is_file() {
                Ok(ProjectRelativePathResolution::Existing {
                    absolute: candidate,
                    relative,
                })
            } else {
                Ok(ProjectRelativePathResolution::NotFile {
                    absolute: candidate,
                    relative,
                })
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let resolved_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let mut ancestor = candidate.parent();
            while let Some(path) = ancestor {
                match path.canonicalize() {
                    Ok(resolved) => {
                        if workspace_relative_path(&resolved_root, &resolved).is_none() {
                            return Ok(ProjectRelativePathResolution::OutOfProject);
                        }
                        break;
                    }
                    Err(ancestor_error) if ancestor_error.kind() == io::ErrorKind::NotFound => {
                        ancestor = path.parent();
                    }
                    Err(ancestor_error) => return Err(ancestor_error),
                }
            }
            Ok(ProjectRelativePathResolution::Missing {
                absolute: candidate,
                relative,
            })
        }
        Err(error) => Err(error),
    }
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

    #[test]
    fn exact_project_path_resolution_distinguishes_existing_missing_and_outside() -> Result<()> {
        let project = tempdir()?;
        let source = project.path().join("src").join("lib.rs");
        fs::create_dir_all(source.parent().expect("source parent"))?;
        fs::write(&source, "pub fn exact() {}\n")?;

        assert_eq!(
            resolve_project_relative_path(project.path(), Path::new("src/./lib.rs"))?,
            ProjectRelativePathResolution::Existing {
                absolute: source.clone(),
                relative: PathBuf::from("src/lib.rs"),
            }
        );
        assert_eq!(
            resolve_project_relative_path(project.path(), Path::new("src/missing.rs"))?,
            ProjectRelativePathResolution::Missing {
                absolute: project.path().join("src").join("missing.rs"),
                relative: PathBuf::from("src/missing.rs"),
            }
        );
        assert_eq!(
            resolve_project_relative_path(project.path(), Path::new("../outside.rs"))?,
            ProjectRelativePathResolution::OutOfProject
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn exact_project_path_resolution_rejects_outside_symlink_targets() -> Result<()> {
        use std::os::unix::fs::symlink;

        let project = tempdir()?;
        let outside = tempdir()?;
        let target = outside.path().join("target.rs");
        fs::write(&target, "pub fn outside() {}\n")?;
        let link = project.path().join("inside.rs");
        symlink(&target, &link)?;

        assert_eq!(
            resolve_project_relative_path(project.path(), Path::new("inside.rs"))?,
            ProjectRelativePathResolution::OutOfProject
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn missing_leaf_beneath_outside_symlink_is_out_of_project() -> Result<()> {
        use std::os::unix::fs::symlink;

        let project = tempdir()?;
        let outside = tempdir()?;
        let link = project.path().join("linked-out");
        symlink(outside.path(), &link)?;

        assert_eq!(
            resolve_project_relative_path(project.path(), Path::new("linked-out/missing.rs"))?,
            ProjectRelativePathResolution::OutOfProject
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn exact_project_path_resolution_accepts_a_native_project_alias() -> Result<()> {
        use std::os::unix::fs::symlink;

        let parent = tempdir()?;
        let project = parent.path().join("project");
        let alias = parent.path().join("project-alias");
        let source = project.join("src").join("lib.rs");
        fs::create_dir_all(source.parent().expect("source parent"))?;
        fs::write(&source, "pub fn exact() {}\n")?;
        symlink(&project, &alias)?;

        assert_eq!(
            resolve_project_relative_path(&alias, Path::new("src/lib.rs"))?,
            ProjectRelativePathResolution::Existing {
                absolute: alias.join("src").join("lib.rs"),
                relative: PathBuf::from("src/lib.rs"),
            }
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn exact_project_path_resolution_uses_windows_native_alias_identity() -> Result<()> {
        let parent = tempdir()?;
        let project = parent.path().join("CodeStory");
        let source = project.join("src").join("lib.rs");
        fs::create_dir_all(source.parent().expect("source parent"))?;
        fs::write(&source, "pub fn exact() {}\n")?;
        let alias = parent.path().join("codestory").join("SRC").join("LIB.RS");

        assert_eq!(
            resolve_project_relative_path(&project, &alias)?,
            ProjectRelativePathResolution::Existing {
                absolute: alias,
                relative: PathBuf::from("SRC").join("LIB.RS"),
            }
        );
        Ok(())
    }

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
                policy_exclusions: Vec::new(),
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert_eq!(plan.mode, RefreshMode::Incremental);
        assert_eq!(plan.files_to_index, vec![file]);
        assert!(plan.files_to_remove.is_empty());
        Ok(())
    }

    #[test]
    fn structural_exclusion_uses_only_workspace_relative_descendants() -> Result<()> {
        let temp = tempdir()?;
        for ancestor in ["build", "target", "vendor", "secrets"] {
            let root = temp.path().join(ancestor).join("repo");
            fs::create_dir_all(&root)?;
            let root_source = root.join("config.json");
            fs::write(&root_source, "{\"root\":true}\n")?;
            for excluded_dir in ["build", "target", "vendor", "secrets"] {
                let excluded = root.join(excluded_dir).join("config.json");
                fs::create_dir_all(excluded.parent().expect("excluded parent"))?;
                fs::write(excluded, "{\"excluded\":true}\n")?;
            }

            let manifest = WorkspaceManifest::open(root.clone())?;
            let inventory = manifest.source_inventory()?;
            assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
            assert!(
                inventory.files.contains(&root_source),
                "workspace ancestor `{ancestor}` excluded the root source"
            );
            for excluded_dir in ["build", "target", "vendor", "secrets"] {
                assert!(
                    !inventory
                        .files
                        .contains(&root.join(excluded_dir).join("config.json")),
                    "workspace-relative `{excluded_dir}` descendant remained admitted"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn excluded_structural_inventory_entry_schedules_pre_policy_row_removal() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let excluded = root.join("vendor/config.json");
        fs::create_dir_all(excluded.parent().expect("excluded parent"))?;
        fs::write(&excluded, "{\"legacy\":true}\n")?;
        let manifest = WorkspaceManifest::open(root)?;
        let outcome = WorkspaceDiscovery.build_refresh_outcome(
            &manifest,
            &RefreshInputs {
                stored_files: Vec::new(),
                policy_exclusions: Vec::new(),
                inventory: WorkspaceInventory::from_records([(
                    excluded,
                    IndexedFileRecord {
                        file_id: 44,
                        modification_time: 0,
                        content_hash: Some("a".repeat(64)),
                        indexed: true,
                        complete: true,
                        retry_required: false,
                    },
                )]),
            },
        )?;

        assert_eq!(
            outcome.inventory_outcome,
            WorkspaceInventoryOutcome::Complete
        );
        assert!(outcome.plan.files_to_index.is_empty());
        assert_eq!(outcome.plan.files_to_remove, vec![44]);
        assert!(outcome.plan.existing_file_ids.is_empty());
        Ok(())
    }

    #[test]
    fn declared_json_lockfiles_are_excluded_without_hiding_ordinary_json() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let lockfile = root.join("skills-lock.json");
        let ordinary = root.join("config.json");
        fs::write(&lockfile, "{\"skills\":[]}\n")?;
        fs::write(&ordinary, "{\"enabled\":true}\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
        assert!(!inventory.files.contains(&lockfile));
        assert!(inventory.files.contains(&ordinary));
        Ok(())
    }

    #[test]
    fn classifies_generated_vendor_and_ordinary_oversized_sources_before_scheduling() -> Result<()>
    {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("generated"))?;
        fs::create_dir_all(root.join("vendor"))?;
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("generated/registers.h"), vec![b'g'; 96])?;
        fs::write(root.join("vendor/bundle.js"), vec![b'v'; 80])?;
        fs::write(root.join("src/ordinary.rs"), vec![b'o'; 65])?;
        fs::write(root.join("src/small.rs"), b"fn small() {}\n")?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let inventory =
            WorkspaceDiscovery.source_inventory_with_policy(&manifest, 64, "test-policy-v1")?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
        assert_eq!(inventory.files, vec![root.join("src/small.rs")]);
        assert_eq!(inventory.policy_exclusions.len(), 3);
        assert_eq!(
            inventory
                .policy_exclusions
                .iter()
                .map(|entry| entry.normalized_path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "generated/registers.h",
                "src/ordinary.rs",
                "vendor/bundle.js"
            ]
        );
        assert!(inventory.policy_exclusions.iter().all(|entry| {
            entry.content_hash.len() == 64
                && entry.observed_size > entry.byte_cap
                && entry.policy_version == "test-policy-v1"
        }));
        Ok(())
    }

    #[test]
    fn changed_oversized_content_produces_a_new_verified_identity() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        let oversized = root.join("src/large.rs");
        fs::write(&oversized, vec![b'a'; 65])?;
        let manifest = WorkspaceManifest::open(root)?;
        let first =
            WorkspaceDiscovery.source_inventory_with_policy(&manifest, 64, "test-policy-v1")?;
        fs::write(&oversized, vec![b'b'; 66])?;
        let second =
            WorkspaceDiscovery.source_inventory_with_policy(&manifest, 64, "test-policy-v1")?;
        let changed_policy =
            WorkspaceDiscovery.source_inventory_with_policy(&manifest, 64, "test-policy-v2")?;

        assert_ne!(
            first.policy_exclusions[0].content_hash,
            second.policy_exclusions[0].content_hash
        );
        assert_eq!(second.policy_exclusions[0].observed_size, 66);
        assert_eq!(
            changed_policy.policy_exclusions[0].content_hash,
            second.policy_exclusions[0].content_hash
        );
        assert_eq!(
            changed_policy.policy_exclusions[0].policy_version,
            "test-policy-v2"
        );
        Ok(())
    }

    #[test]
    fn refresh_carries_forward_unchanged_structural_unit_exclusions() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let path = root.join("evidence.json");
        fs::write(&path, "{\"one\":1,\"two\":2,\"three\":3}\n")?;
        let policy = SourceIndexPolicy {
            policy_version: "test-structural-policy-v1".to_string(),
            byte_cap: 1_000,
            structural_unit_cap: 2,
        };
        let (content_hash, observed_size) = current_content_identity(&path)?;
        let retained = OversizedSourceExclusionCandidate {
            normalized_path: "evidence.json".to_string(),
            content_hash,
            observed_size,
            observed_unit_count: 3,
            policy_version: policy.policy_version.clone(),
            byte_cap: policy.byte_cap,
            structural_unit_cap: policy.structural_unit_cap,
        };
        let manifest = WorkspaceManifest::open(root)?;
        let inputs = RefreshInputs {
            stored_files: Vec::new(),
            policy_exclusions: vec![retained.clone()],
            inventory: WorkspaceInventory::default(),
        };

        let unchanged = manifest.build_execution_outcome_with_policy(&inputs, &policy)?;
        assert!(unchanged.refresh.plan.files_to_index.is_empty());
        assert_eq!(unchanged.policy_exclusions, vec![retained.clone()]);

        fs::write(&path, "{\"one\":1,\"two\":2,\"three\":4}\n")?;
        let changed = manifest.build_execution_outcome_with_policy(&inputs, &policy)?;
        assert_eq!(changed.refresh.plan.files_to_index, vec![path]);
        assert!(changed.policy_exclusions.is_empty());
        Ok(())
    }

    #[test]
    fn partial_discovery_never_classifies_a_deletion_capable_exclusion_set() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        let oversized = root.join("src/large.rs");
        fs::write(&oversized, vec![b'a'; 65])?;
        write_repo_manifest(&root, vec![PathBuf::from("src"), PathBuf::from("missing")])?;
        let manifest = WorkspaceManifest::open(root)?;
        let inventory =
            WorkspaceDiscovery.source_inventory_with_policy(&manifest, 64, "test-policy-v1")?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Partial);
        assert!(inventory.policy_exclusions.is_empty());
        assert_eq!(inventory.files, vec![oversized]);
        assert!(!inventory.issues.is_empty());
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
                policy_exclusions: Vec::new(),
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
                policy_exclusions: Vec::new(),
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert_eq!(plan.files_to_index, vec![file]);
        Ok(())
    }

    fn indexed_inputs_for(files: &[PathBuf]) -> Result<RefreshInputs> {
        let mut stored_files = Vec::new();
        for (index, file) in files.iter().enumerate() {
            stored_files.push(StoredFileState {
                id: index as i64 + 1,
                path: file.clone(),
                modification_time: modification_time_millis(file)?,
                content_hash: Some(current_content_hash(file)?),
                indexed: true,
                complete: true,
                retry_required: false,
            });
        }
        Ok(RefreshInputs {
            stored_files,
            policy_exclusions: Vec::new(),
            inventory: WorkspaceInventory::default(),
        })
    }

    /// One public operation derives source freshness several times (before and
    /// after the build, again for the second public-operation wrapper, again
    /// for strict retrieval readiness). Without the operation-scoped verdict
    /// memo each pass re-hashes every unchanged file.
    #[test]
    fn repeated_refresh_planning_in_one_scope_hashes_each_file_once() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let files = ["a.rs", "b.rs", "c.rs"]
            .iter()
            .map(|name| {
                let path = root.join(name);
                fs::write(
                    &path,
                    format!("fn {}() {{}}\n", name.trim_end_matches(".rs")),
                )?;
                Ok(path)
            })
            .collect::<Result<Vec<_>>>()?;
        let inputs = indexed_inputs_for(&files)?;
        let manifest = WorkspaceManifest::open(root)?;

        let scope = SourceFreshnessScope::enter();
        for _ in 0..4 {
            let plan = WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
            assert!(
                plan.files_to_index.is_empty(),
                "unchanged files must stay clean across every pass"
            );
        }
        let counts = source_freshness_counts().expect("an armed scope reports counts");
        assert_eq!(
            counts.content_hash_reads, 3,
            "four freshness passes over three unchanged files must read content exactly once per file"
        );
        assert_eq!(counts.verdict_reuses, 9);
        drop(scope);

        Ok(())
    }

    /// The memo must not survive the operation that produced it: the next
    /// operation re-verifies from content.
    #[test]
    fn a_new_scope_reverifies_content_instead_of_reusing_the_previous_verdict() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let inputs = indexed_inputs_for(std::slice::from_ref(&file))?;
        let manifest = WorkspaceManifest::open(root)?;

        {
            let _first = SourceFreshnessScope::enter();
            WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
            assert_eq!(
                source_freshness_counts()
                    .expect("armed scope")
                    .content_hash_reads,
                1
            );
        }
        {
            let _second = SourceFreshnessScope::enter();
            WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
            let counts = source_freshness_counts().expect("armed scope");
            assert_eq!(
                counts.content_hash_reads, 1,
                "a new operation must hash the file again"
            );
            assert_eq!(counts.verdict_reuses, 0);
        }
        Ok(())
    }

    /// Same-mtime drift that changes the file's length is caught even inside a
    /// single armed scope, because the observed length is part of the memo key.
    #[test]
    fn same_mtime_drift_that_changes_length_is_detected_inside_one_scope() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let inputs = indexed_inputs_for(std::slice::from_ref(&file))?;
        let manifest = WorkspaceManifest::open(root)?;
        let original_mtime = fs::metadata(&file)?.modified()?;

        let _scope = SourceFreshnessScope::enter();
        let clean = WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
        assert!(clean.files_to_index.is_empty());

        fs::write(&file, "fn main() { let drifted = 1; }\n")?;
        fs::File::open(&file)?.set_modified(original_mtime)?;
        assert_eq!(
            fs::metadata(&file)?.modified()?,
            original_mtime,
            "the drift must preserve the modification time to exercise the guard"
        );

        let drifted = WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
        assert_eq!(
            drifted.files_to_index,
            vec![file],
            "a length change is a positive drift signal the memo key must not absorb"
        );
        Ok(())
    }

    /// Drift that preserves BOTH the modification time and the byte length is
    /// invisible to metadata, so the content hash is the only mechanism that
    /// sees it. A memoized verdict describes the instant it was taken at, so a
    /// derivation that must see drift *since* then reverifies from content
    /// first — and after that call the plan must name the drifted file again.
    #[test]
    fn same_mtime_same_length_drift_is_detected_after_reverification() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let inputs = indexed_inputs_for(std::slice::from_ref(&file))?;
        let manifest = WorkspaceManifest::open(root)?;
        let original_mtime = fs::metadata(&file)?.modified()?;
        let original_len = fs::metadata(&file)?.len();

        let _scope = SourceFreshnessScope::enter();
        let clean = WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
        assert!(clean.files_to_index.is_empty());

        // Same length, same mtime, different bytes: exactly the coarse-mtime /
        // mtime-preserving-tool case the content hash exists to catch.
        fs::write(&file, "fn maim() {}\n")?;
        fs::File::open(&file)?.set_modified(original_mtime)?;
        let observed = fs::metadata(&file)?;
        assert_eq!(
            observed.len(),
            original_len,
            "the drift must preserve the byte length to exercise the guard"
        );
        assert_eq!(
            observed.modified()?,
            original_mtime,
            "the drift must preserve the modification time to exercise the guard"
        );

        source_freshness::reverify_from_content();

        let drifted = WorkspaceDiscovery.build_refresh_plan(&manifest, &inputs)?;
        assert_eq!(
            drifted.files_to_index,
            vec![file],
            "reverification must re-read content, which is the only thing that \
             sees same-mtime same-length drift"
        );
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
                policy_exclusions: Vec::new(),
                inventory: WorkspaceInventory::default(),
            },
            RefreshInputs {
                stored_files: Vec::new(),
                policy_exclusions: Vec::new(),
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
                policy_exclusions: Vec::new(),
                inventory: WorkspaceInventory::default(),
            },
            RefreshInputs {
                stored_files: Vec::new(),
                policy_exclusions: Vec::new(),
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
                policy_exclusions: Vec::new(),
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
            policy_exclusions: Vec::new(),
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
            policy_exclusions: Vec::new(),
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
            policy_exclusions: Vec::new(),
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
                policy_exclusions: Vec::new(),
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
    fn caller_owned_generated_roots_exclude_descendants_without_hiding_siblings() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let generated = root.join("cache").join("search-generations");
        let sibling = root.join("cache").join("search-generations-user");
        fs::create_dir_all(&generated)?;
        fs::create_dir_all(&sibling)?;
        fs::write(root.join("config.json"), "{\"indexed\":true}\n")?;
        fs::write(generated.join("meta.json"), "{\"generated\":true}\n")?;
        fs::write(sibling.join("config.json"), "{\"indexed\":true}\n")?;

        let mut manifest = WorkspaceManifest::open(root.clone())?;
        manifest.exclude_discovery_directory_roots([generated.clone()]);
        let files = manifest.source_files()?;

        assert!(files.contains(&root.join("config.json")));
        assert!(files.contains(&sibling.join("config.json")));
        assert!(!files.contains(&generated.join("meta.json")));
        Ok(())
    }

    #[test]
    fn caller_owned_file_identity_excludes_hardlink_aliases() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let owned = root.join("cache").join("codestory.db");
        let alias = root.join("looks-like-user-source.json");
        fs::create_dir_all(owned.parent().expect("owned parent"))?;
        fs::write(&owned, "{\"generated\":true}\n")?;
        fs::hard_link(&owned, &alias)?;
        fs::write(root.join("config.json"), "{\"indexed\":true}\n")?;

        let mut manifest = WorkspaceManifest::open(root.clone())?;
        manifest.exclude_discovery_files([owned]);
        let files = manifest.source_files()?;

        assert!(!files.contains(&alias));
        assert!(files.contains(&root.join("config.json")));

        let mut direct = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::Json,
                    standard: LanguageStandard::Default,
                    source_paths: vec![alias],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );
        direct.exclude_discovery_files([root.join("cache").join("codestory.db")]);
        let direct_inventory = direct.source_inventory()?;
        assert_eq!(
            direct_inventory.outcome,
            WorkspaceInventoryOutcome::Complete
        );
        assert!(direct_inventory.files.is_empty());
        Ok(())
    }

    #[test]
    fn caller_owned_missing_file_uses_platform_lexical_identity() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("cache"))?;
        let missing = root.join("cache").join("codestory.db-wal");
        let dotted = root
            .join("cache")
            .join("nested")
            .join("..")
            .join("codestory.db-wal");
        assert_eq!(
            workspace_path_identity(&missing)?,
            workspace_path_identity(&dotted)?
        );

        #[cfg(windows)]
        assert_eq!(
            workspace_path_identity(&missing)?,
            workspace_path_identity(&root.join("CACHE").join("CODESTORY.DB-WAL"))?
        );
        #[cfg(unix)]
        assert_ne!(
            workspace_path_identity(&missing)?,
            workspace_path_identity(&root.join("cache").join("CODESTORY.DB-WAL"))?
        );
        Ok(())
    }

    #[test]
    fn caller_owned_directory_excludes_a_direct_configured_source() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let generated = root.join("cache").join("search-generations");
        fs::create_dir_all(&generated)?;
        fs::write(generated.join("meta.json"), "{\"generated\":true}\n")?;

        let mut manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::Json,
                    standard: LanguageStandard::Default,
                    source_paths: vec![generated.clone()],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );
        manifest.exclude_discovery_directory_roots([generated]);
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
        assert!(inventory.files.is_empty());
        Ok(())
    }

    #[test]
    fn caller_owned_exclusion_roots_are_observed_once_per_inventory() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        for index in 0..256 {
            fs::write(
                root.join(format!("config-{index}.json")),
                format!("{{\"index\":{index}}}\n"),
            )?;
        }

        let storage_path = root.join("cache/custom-core.db");
        let manifest =
            WorkspaceManifest::open_with_storage_owned_exclusions(root.clone(), &storage_path)?;
        assert_eq!(
            legacy_search_directory_for_storage(&storage_path),
            root.join("cache/custom-core.search")
        );
        assert_eq!(
            search_generation_directory_for_storage(&storage_path),
            root.join("cache/custom-core.search-generations")
        );
        let files = manifest.source_files()?;

        assert_eq!(files.len(), 256);
        assert_eq!(
            manifest.discovery_exclusion_observation_count(),
            storage_owned_discovery_files(&storage_path).len()
                + storage_owned_discovery_directory_roots(&storage_path).len()
        );
        Ok(())
    }

    #[test]
    fn storage_owned_artifacts_stay_excluded_while_hidden_user_sources_remain_visible() -> Result<()>
    {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let storage_path = root.join(".cache/custom-core.db");
        let hidden_source = root.join(".github/workflows/ci.yml");
        fs::create_dir_all(storage_path.parent().expect("storage parent"))?;
        fs::create_dir_all(hidden_source.parent().expect("hidden source parent"))?;
        fs::write(&hidden_source, "name: CI\non: push\njobs: {}\n")?;

        let owned_files = storage_owned_discovery_files(&storage_path);
        for path in &owned_files {
            fs::write(path, b"codestory-owned\n")?;
        }
        let staged_files = [
            root.join(".cache/custom-core.staged.123-456.db"),
            root.join(".cache/custom-core.staged.123-456.db-wal"),
            root.join(".cache/custom-core.staged.123-456.db-shm"),
            root.join(".cache/custom-core.staged.123-456.db-journal"),
        ];
        for path in &staged_files {
            fs::write(path, b"codestory-owned stage\n")?;
        }
        let staged_near_miss = root.join(".cache/custom-core.staged.notes.db");
        fs::write(&staged_near_miss, b"user-owned\n")?;

        let manifest = WorkspaceManifest::open_with_storage_owned_exclusions(root, &storage_path)?;
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
        assert!(inventory.files.contains(&hidden_source));
        assert!(inventory.files.contains(&staged_near_miss));
        assert!(
            owned_files
                .iter()
                .all(|owned| !inventory.files.contains(owned))
        );
        assert!(
            staged_files
                .iter()
                .all(|owned| !inventory.files.contains(owned))
        );
        Ok(())
    }

    #[test]
    fn quarantined_derived_reset_state_stays_out_of_source_discovery() -> Result<()> {
        // The guided derived-cache reset moves owned artifacts into a
        // quarantine tree inside the cache root instead of deleting them, so
        // every file it reclaimed is still on disk under a name discovery has
        // never seen. Unless the quarantine root is an owned directory
        // identity, the next inventory adopts the whole reclaimed cache as
        // user source.
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let storage_path = root.join(".cache/custom-core.db");
        let quarantine =
            codestory_contracts::owned_artifacts::derived_reset_quarantine_root(&storage_path)
                .join("0001700000000");
        fs::create_dir_all(quarantine.join("custom-core.search"))?;
        let quarantined = [
            quarantine.join("custom-core.db"),
            quarantine.join("custom-core.db-wal"),
            quarantine.join("local-refresh-status.json"),
            quarantine.join("custom-core.search").join("meta.json"),
        ];
        for path in &quarantined {
            fs::write(path, b"quarantined derived state\n")?;
        }
        let user_source = root.join("src/lib.rs");
        fs::create_dir_all(user_source.parent().expect("user source parent"))?;
        fs::write(&user_source, "pub fn keep() {}\n")?;

        let manifest = WorkspaceManifest::open_with_storage_owned_exclusions(root, &storage_path)?;
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
        assert!(inventory.files.contains(&user_source));
        for path in &quarantined {
            assert!(
                !inventory.files.contains(path),
                "{} was quarantined by the derived-cache reset and must never be discovered as user source",
                path.display()
            );
        }
        Ok(())
    }

    #[test]
    fn bare_storage_path_observes_staged_siblings_from_current_directory() {
        assert_eq!(
            storage_parent_for_observation(Path::new("codestory.db")),
            Path::new(".")
        );
    }

    #[cfg(unix)]
    #[test]
    fn storage_owned_sqlite_sidecars_preserve_non_utf8_path_bytes() -> Result<()> {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let storage_path = root.join(std::ffi::OsString::from_vec(b"core-\xff.db".to_vec()));
        let native_wal = path_with_native_suffix(&storage_path, "-wal");
        let lossy_user_file = root.join("core-�.db-wal");

        let manifest = WorkspaceManifest::open_with_storage_owned_exclusions(root, &storage_path)?;
        let exclusions =
            observe_discovery_exclusions(&manifest).expect("observe storage-owned exclusions");

        assert!(exclusions.file_is_excluded(&native_wal)?);
        assert!(!exclusions.file_is_excluded(&lossy_user_file)?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unavailable_caller_owned_exclusion_fails_inventory_closed() -> Result<()> {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        fs::write(root.join("config.json"), "{\"indexed\":true}\n")?;
        let unavailable = root.join(std::ffi::OsString::from_vec(b"bad\0root".to_vec()));

        let mut manifest = WorkspaceManifest::open(root)?;
        manifest.exclude_discovery_directory_roots([unavailable.clone()]);
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Unreadable);
        assert!(inventory.files.is_empty());
        assert_eq!(inventory.issues.len(), 1);
        assert_eq!(inventory.issues[0].path, unavailable);
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
    fn synthetic_manifest_discovers_hidden_sources_but_excludes_git_metadata() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let workflow = root.join(".github/workflows/ci.yml");
        let git_source = root.join(".git/objects/should-not-index.rs");
        fs::create_dir_all(workflow.parent().expect("workflow parent"))?;
        fs::create_dir_all(git_source.parent().expect("git source parent"))?;
        fs::write(&workflow, "name: CI\non: push\njobs: {}\n")?;
        fs::write(&git_source, "pub fn repository_metadata() {}\n")?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let inventory = manifest.source_inventory()?;

        assert_eq!(inventory.outcome, WorkspaceInventoryOutcome::Complete);
        assert!(inventory.files.contains(&workflow));
        assert!(!inventory.files.contains(&git_source));
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
            Language::Shell,
            Language::PowerShell,
            Language::Markdown,
            Language::Yaml,
            Language::Toml,
            Language::Json,
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

    /// The registry-driven companion match must accept exactly the pairs the
    /// hand-written cross product accepted — no more, no fewer.
    ///
    /// The positive direction alone is not enough: routing this through a
    /// registry could widen discovery (a `.scss` file entering the JavaScript
    /// source group, say), and discovery admission decides what gets indexed.
    /// The expected set below is the pre-move table restated literally.
    #[test]
    fn companion_extension_source_groups_match_the_hand_written_cross_product() {
        const EXPECTED: &[(&str, &[&str])] = &[
            ("vue", &["JavaScript", "TypeScript", "Vue"]),
            ("svelte", &["JavaScript", "TypeScript", "Svelte"]),
            ("astro", &["JavaScript", "TypeScript", "Astro"]),
            ("cshtml", &["CSharp"]),
            ("lua", &["Lua"]),
            ("scss", &["Css"]),
            ("sass", &["Css"]),
            ("less", &["Css"]),
        ];
        let all_languages = [
            ("Cxx", Language::Cxx),
            ("Java", Language::Java),
            ("Python", Language::Python),
            ("Rust", Language::Rust),
            ("JavaScript", Language::JavaScript),
            ("TypeScript", Language::TypeScript),
            ("Go", Language::Go),
            ("Ruby", Language::Ruby),
            ("Php", Language::Php),
            ("CSharp", Language::CSharp),
            ("Kotlin", Language::Kotlin),
            ("Swift", Language::Swift),
            ("Dart", Language::Dart),
            ("Lua", Language::Lua),
            ("Sql", Language::Sql),
            ("Html", Language::Html),
            ("Css", Language::Css),
            ("Bash", Language::Bash),
            ("Shell", Language::Shell),
            ("PowerShell", Language::PowerShell),
            ("Markdown", Language::Markdown),
            ("Yaml", Language::Yaml),
            ("Toml", Language::Toml),
            ("Json", Language::Json),
            ("Svelte", Language::Svelte),
            ("Vue", Language::Vue),
            ("Astro", Language::Astro),
        ];

        let declared = codestory_contracts::language_support::COMPANION_EXTENSION_PROFILES
            .iter()
            .map(|profile| profile.extension)
            .collect::<Vec<_>>();
        let expected_extensions = EXPECTED
            .iter()
            .map(|(extension, _)| *extension)
            .collect::<Vec<_>>();
        assert_eq!(
            declared, expected_extensions,
            "the companion registry gained or lost an extension"
        );

        for (extension, accepted) in EXPECTED {
            for (name, language) in &all_languages {
                let expected = accepted.contains(name);
                assert_eq!(
                    compatibility_extension_matches_source_group(extension, language),
                    expected,
                    "`{extension}` in the {name} source group"
                );
                // The same answer must hold through the public entry point, so
                // the registry cannot leak in via the other branch either.
                let file_name = format!("main.{extension}");
                assert_eq!(
                    matches_source_group_language(Path::new(&file_name), language),
                    expected,
                    "`main.{extension}` discovered for {name}"
                );
            }
        }

        // Extensions outside the companion table stay outside it.
        for extension in ["kt", "rs", "txt", "cshtmlx", ""] {
            for (_, language) in &all_languages {
                assert!(
                    !compatibility_extension_matches_source_group(extension, language),
                    "`{extension}` must not be a companion extension"
                );
            }
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

    #[test]
    fn filesystem_epoch_millis_clamps_pre_epoch_and_saturates() {
        let before_epoch = std::time::UNIX_EPOCH
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("represent pre-epoch timestamp");
        assert_eq!(clamp_system_time_to_epoch_millis(before_epoch), 0);
        assert_eq!(
            clamp_system_time_to_epoch_millis(
                std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_234),
            ),
            1_234
        );
    }
}
