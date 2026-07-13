//! Bounded, post-publication retention for one sidecar namespace.

use crate::config::{SidecarLayout, SidecarProfile, SidecarRuntimeConfig};
use crate::qdrant_client::{QdrantClient, QdrantDeleteOutcome};
use anyhow::{Context, Result, bail};
use codestory_store::{RetrievalIndexManifest, Store};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const RETENTION_SCHEMA_VERSION: u32 = 1;
const RETENTION_DIR: &str = "retention";
pub const GLOBAL_GENERATION_GC_LOCK_SCOPE: &str = "global_generation_gc";

pub fn global_generation_gc_state_file(runtime: &SidecarRuntimeConfig) -> PathBuf {
    let base = match runtime.profile {
        SidecarProfile::Local => runtime.layout.state_file.parent(),
        SidecarProfile::Agent => runtime
            .layout
            .state_file
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent),
    }
    .unwrap_or_else(|| Path::new("."));
    base.join("generation-retention-coordination.state")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedRollbackManifest {
    pub manifest: RetrievalIndexManifest,
    pub verified_at_epoch_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRetentionMarker {
    pub schema_version: u32,
    pub workspace_id: String,
    pub project_id: String,
    pub active: RetrievalIndexManifest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback: Option<VerifiedRollbackManifest>,
    pub updated_at_epoch_ms: i64,
}

impl GenerationRetentionMarker {
    pub fn next(
        workspace_id: &str,
        active: RetrievalIndexManifest,
        verified_previous: Option<VerifiedRollbackManifest>,
        updated_at_epoch_ms: i64,
    ) -> Result<Self> {
        validate_retention_component(workspace_id)?;
        let active_generation = canonical_manifest_generation(&active)?;
        let rollback = verified_previous.filter(|rollback| {
            rollback.manifest.project_id == active.project_id
                && canonical_manifest_generation(&rollback.manifest)
                    .ok()
                    .is_some_and(|generation| generation != active_generation)
        });
        let marker = Self {
            schema_version: RETENTION_SCHEMA_VERSION,
            workspace_id: workspace_id.to_string(),
            project_id: active.project_id.clone(),
            active,
            rollback,
            updated_at_epoch_ms,
        };
        validate_marker(&marker)?;
        Ok(marker)
    }
}

pub struct GenerationRetentionLock {
    file: File,
}

impl GenerationRetentionLock {
    pub fn acquire(state_file: &Path, scope_id: &str) -> Result<Self> {
        Self::acquire_with_mode(state_file, scope_id, false)
    }

    pub fn acquire_shared(state_file: &Path, scope_id: &str) -> Result<Self> {
        Self::acquire_with_mode(state_file, scope_id, true)
    }

    pub fn try_acquire_shared(state_file: &Path, scope_id: &str) -> Result<Option<Self>> {
        let path = retention_lock_path(state_file, scope_id)?;
        ensure_retention_dir(state_file)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open generation retention lock {}", path.display()))?;
        match FileExt::try_lock_shared(&file) {
            Ok(true) => Ok(Some(Self { file })),
            Ok(false) => Ok(None),
            Err(error) => Err(error).with_context(|| {
                format!("try lock shared generation retention {}", path.display())
            }),
        }
    }

    fn acquire_with_mode(state_file: &Path, scope_id: &str, shared: bool) -> Result<Self> {
        let path = retention_lock_path(state_file, scope_id)?;
        ensure_retention_dir(state_file)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open generation retention lock {}", path.display()))?;
        if shared {
            FileExt::lock_shared(&file)
                .with_context(|| format!("lock shared generation retention {}", path.display()))?;
        } else {
            FileExt::lock_exclusive(&file)
                .with_context(|| format!("lock generation retention {}", path.display()))?;
        }
        Ok(Self { file })
    }
}

impl Drop for GenerationRetentionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn retention_marker_path(state_file: &Path, workspace_id: &str) -> Result<PathBuf> {
    validate_retention_component(workspace_id)?;
    Ok(retention_dir(state_file).join(format!("{workspace_id}.json")))
}

pub fn retention_lock_path(state_file: &Path, scope_id: &str) -> Result<PathBuf> {
    validate_retention_component(scope_id)?;
    Ok(retention_dir(state_file).join(format!("{scope_id}.lock")))
}

pub fn read_retention_marker(
    state_file: &Path,
    workspace_id: &str,
) -> Result<Option<GenerationRetentionMarker>> {
    let path = retention_marker_path(state_file, workspace_id)?;
    read_marker_path(&path)
}

pub fn write_retention_marker(
    state_file: &Path,
    marker: &GenerationRetentionMarker,
) -> Result<PathBuf> {
    validate_marker(marker)?;
    ensure_retention_dir(state_file)?;
    let path = retention_marker_path(state_file, &marker.workspace_id)?;
    let bytes =
        serde_json::to_vec_pretty(marker).context("serialize generation retention marker")?;
    codestory_workspace::atomic_file::write_file_atomic(
        &path,
        "generation-retention",
        |file| {
            use std::io::Write;
            file.write_all(&bytes)
                .context("write generation retention marker")
        },
        |temp_path| {
            let candidate: GenerationRetentionMarker = serde_json::from_slice(
                &std::fs::read(temp_path).context("read temporary retention marker")?,
            )
            .context("parse temporary retention marker")?;
            validate_marker(&candidate)?;
            if &candidate != marker {
                bail!("temporary generation retention marker differs from expected marker");
            }
            Ok(())
        },
    )
    .with_context(|| format!("publish generation retention marker {}", path.display()))?;
    Ok(path)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionProtectionScan {
    /// Manifests from the storage path explicitly requested by the caller.
    pub authoritative_active: Vec<RetrievalIndexManifest>,
    /// A rollback re-verified during the current publication, when available.
    pub authoritative_rollback: Vec<RetrievalIndexManifest>,
    /// Other manifest-referenced active generations sharing the sidecar scope.
    pub active: Vec<RetrievalIndexManifest>,
    pub rollback: Vec<RetrievalIndexManifest>,
    pub storage_paths_scanned: Vec<PathBuf>,
    pub marker_paths_scanned: Vec<PathBuf>,
    pub errors: Vec<String>,
}

pub fn scan_retention_protection(
    cache_root: &Path,
    active_storage_path: Option<&Path>,
    state_file: &Path,
) -> RetentionProtectionScan {
    let mut scan = RetentionProtectionScan::default();
    let storage_paths = storage_paths_for_scan(cache_root, active_storage_path, &mut scan.errors);
    for storage_path in storage_paths {
        match Store::open(&storage_path).and_then(|store| store.list_retrieval_index_manifests()) {
            Ok(manifests) => {
                if active_storage_path.is_some_and(|active| active == storage_path) {
                    scan.authoritative_active.extend(manifests.clone());
                }
                scan.storage_paths_scanned.push(storage_path);
                scan.active.extend(manifests);
            }
            Err(error) => scan.errors.push(format!(
                "scan retrieval manifests in {}: {error}",
                storage_path.display()
            )),
        }
    }

    let marker_dir = retention_dir(state_file);
    match std::fs::symlink_metadata(&marker_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            scan.errors.push(format!(
                "retention marker path is not a direct directory: {}",
                marker_dir.display()
            ));
        }
        Ok(_) => match std::fs::read_dir(&marker_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            scan.errors.push(format!(
                                "read retention marker entry in {}: {error}",
                                marker_dir.display()
                            ));
                            continue;
                        }
                    };
                    let path = entry.path();
                    if path.extension().and_then(|value| value.to_str()) != Some("json") {
                        continue;
                    }
                    let file_type = match entry.file_type() {
                        Ok(file_type) => file_type,
                        Err(error) => {
                            scan.errors.push(format!(
                                "read retention marker type {}: {error}",
                                path.display()
                            ));
                            continue;
                        }
                    };
                    if file_type.is_symlink() || !file_type.is_file() {
                        scan.errors.push(format!(
                            "retention marker is not a direct regular file: {}",
                            path.display()
                        ));
                        continue;
                    }
                    match read_marker_path(&path) {
                        Ok(Some(marker)) => {
                            scan.marker_paths_scanned.push(path);
                            scan.active.push(marker.active);
                            if let Some(rollback) = marker.rollback {
                                scan.rollback.push(rollback.manifest);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => scan.errors.push(format!(
                            "scan retention marker {}: {error:#}",
                            path.display()
                        )),
                    }
                }
            }
            Err(error) => scan.errors.push(format!(
                "read retention marker directory {}: {error}",
                marker_dir.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => scan.errors.push(format!(
            "inspect retention marker directory {}: {error}",
            marker_dir.display()
        )),
    }
    deduplicate_manifests(&mut scan.active);
    deduplicate_manifests(&mut scan.rollback);
    deduplicate_manifests(&mut scan.authoritative_active);
    deduplicate_manifests(&mut scan.authoritative_rollback);
    scan
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationRetentionState {
    Active,
    Rollback,
    Building,
    Reclaimable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationArtifact {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationBundle {
    pub generation: String,
    pub qdrant_collection: String,
    pub state: GenerationRetentionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical: Option<GenerationArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scip: Option<GenerationArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qdrant: Option<GenerationArtifact>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedGenerationEntry {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRetentionPlan {
    pub dry_run: bool,
    pub project_id: String,
    pub pruning_suppressed: bool,
    pub active_bytes: u64,
    pub rollback_bytes: u64,
    pub building_bytes: u64,
    pub retained_bytes: u64,
    pub reclaimable_bytes: u64,
    pub bundles: Vec<GenerationBundle>,
    pub blocked: Vec<BlockedGenerationEntry>,
    pub errors: Vec<String>,
}

#[cfg(test)]
fn plan_generation_retention(
    layout: &SidecarLayout,
    project_id: &str,
    protection: &RetentionProtectionScan,
) -> GenerationRetentionPlan {
    plan_generation_retention_with_qdrant_collections(layout, project_id, protection, &[])
}

/// Build a dry-run plan, supplementing on-disk Qdrant discovery with names
/// returned by the live Qdrant API.
pub fn plan_generation_retention_with_qdrant_collections(
    layout: &SidecarLayout,
    project_id: &str,
    protection: &RetentionProtectionScan,
    live_qdrant_collections: &[String],
) -> GenerationRetentionPlan {
    plan_generation_retention_with_unrooted_state(
        layout,
        project_id,
        protection,
        live_qdrant_collections,
        GenerationRetentionState::Reclaimable,
    )
}

pub(crate) fn plan_generation_retention_with_unrooted_state(
    layout: &SidecarLayout,
    project_id: &str,
    protection: &RetentionProtectionScan,
    live_qdrant_collections: &[String],
    unrooted_state: GenerationRetentionState,
) -> GenerationRetentionPlan {
    debug_assert!(matches!(
        unrooted_state,
        GenerationRetentionState::Building | GenerationRetentionState::Reclaimable
    ));
    let mut errors = protection.errors.clone();
    let mut blocked = Vec::new();
    let mut builders = BTreeMap::<String, BundleBuilder>::new();
    if direct_directory_exists_or_missing(
        &layout.lexical_data_dir,
        "Lexical data root",
        &mut errors,
    ) {
        discover_generation_dirs(
            &layout.lexical_data_dir.join("shards"),
            project_id,
            ArtifactKind::Lexical,
            &mut builders,
            &mut blocked,
            &mut errors,
        );
    }
    discover_generation_dirs(
        &layout.scip_artifacts_root,
        project_id,
        ArtifactKind::Scip,
        &mut builders,
        &mut blocked,
        &mut errors,
    );
    if direct_directory_exists_or_missing(&layout.qdrant_data_dir, "Qdrant data root", &mut errors)
    {
        discover_qdrant_collections(
            &layout.qdrant_data_dir.join("collections"),
            project_id,
            &mut builders,
            &mut blocked,
            &mut errors,
        );
    }
    discover_live_qdrant_collections(
        live_qdrant_collections,
        project_id,
        &mut builders,
        &mut blocked,
        &mut errors,
    );

    let mut active = BTreeSet::new();
    let mut rollback = BTreeSet::new();
    collect_protected_generations(
        project_id,
        &protection.authoritative_active,
        &mut active,
        &mut errors,
        "active",
    );
    collect_protected_generations(
        project_id,
        &protection.active,
        &mut active,
        &mut errors,
        "shared active",
    );
    collect_protected_generations(
        project_id,
        &protection.rollback,
        &mut rollback,
        &mut errors,
        "shared rollback",
    );
    collect_protected_generations(
        project_id,
        &protection.authoritative_rollback,
        &mut rollback,
        &mut errors,
        "authoritative rollback",
    );
    if active.is_empty() {
        errors.push(format!(
            "no active generation is protected for project {project_id}; pruning suppressed"
        ));
    }
    for generation in active.iter().chain(rollback.iter()) {
        builders.entry(generation.clone()).or_default();
    }

    let effective_unrooted_state = if errors.is_empty() {
        unrooted_state
    } else {
        GenerationRetentionState::Building
    };

    let mut bundles = builders
        .into_iter()
        .map(|(generation, builder)| {
            let state = if active.contains(&generation) {
                GenerationRetentionState::Active
            } else if rollback.contains(&generation) {
                GenerationRetentionState::Rollback
            } else {
                effective_unrooted_state
            };
            builder.finish(project_id, generation, state)
        })
        .collect::<Vec<_>>();
    bundles.sort_by(|left, right| left.generation.cmp(&right.generation));
    let bytes_for = |state| {
        bundles
            .iter()
            .filter(|bundle| bundle.state == state)
            .fold(0_u64, |total, bundle| total.saturating_add(bundle.bytes))
    };
    let active_bytes = bytes_for(GenerationRetentionState::Active);
    let rollback_bytes = bytes_for(GenerationRetentionState::Rollback);
    let building_bytes = bytes_for(GenerationRetentionState::Building);
    let reclaimable_bytes = bytes_for(GenerationRetentionState::Reclaimable);
    let retained_bytes = active_bytes
        .saturating_add(rollback_bytes)
        .saturating_add(building_bytes);

    GenerationRetentionPlan {
        dry_run: true,
        project_id: project_id.to_string(),
        pruning_suppressed: effective_unrooted_state == GenerationRetentionState::Building,
        active_bytes,
        rollback_bytes,
        building_bytes,
        retained_bytes,
        reclaimable_bytes,
        bundles,
        blocked,
        errors,
    }
}

pub trait GenerationRemover {
    fn remove_generation_dir(&mut self, path: &Path) -> Result<()>;
    fn delete_qdrant_collection(&mut self, collection: &str) -> Result<bool>;
}

pub struct FsQdrantGenerationRemover {
    qdrant: QdrantClient,
    collections_dir: PathBuf,
}

impl FsQdrantGenerationRemover {
    pub fn new(layout: &SidecarLayout) -> Self {
        Self {
            qdrant: QdrantClient::new(layout),
            collections_dir: layout.qdrant_data_dir.join("collections"),
        }
    }
}

impl GenerationRemover for FsQdrantGenerationRemover {
    fn remove_generation_dir(&mut self, path: &Path) -> Result<()> {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspect generation directory {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!(
                "refuse to remove non-direct generation directory {}",
                path.display()
            );
        }
        std::fs::remove_dir_all(path)
            .with_context(|| format!("remove generation directory {}", path.display()))
    }

    fn delete_qdrant_collection(&mut self, collection: &str) -> Result<bool> {
        let outcome = self.qdrant.delete_collection_with_outcome(collection)?;
        let path = self.collections_dir.join(collection);
        if outcome == QdrantDeleteOutcome::NotFound && path.exists() {
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("inspect orphan Qdrant collection {}", path.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!(
                    "refuse to remove non-direct orphan Qdrant collection {}",
                    path.display()
                );
            }
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("remove orphan Qdrant collection {}", path.display()))?;
        }
        Ok(!path.exists())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRemovalResult {
    pub generation: String,
    pub qdrant_collection: String,
    pub removed_paths: Vec<PathBuf>,
    pub qdrant_collection_removed: bool,
    pub removed_bytes: u64,
    pub remaining_reclaimable_bytes: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRetentionApplyReport {
    pub dry_run: bool,
    pub project_id: String,
    pub pruning_suppressed: bool,
    pub active_bytes: u64,
    pub rollback_bytes: u64,
    pub building_bytes: u64,
    pub retained_bytes: u64,
    pub reclaimable_bytes: u64,
    pub removed_bytes: u64,
    pub remaining_reclaimable_bytes: u64,
    pub removals: Vec<GenerationRemovalResult>,
    pub errors: Vec<String>,
}

pub fn apply_generation_retention(
    plan: &GenerationRetentionPlan,
    remover: &mut dyn GenerationRemover,
) -> GenerationRetentionApplyReport {
    if plan.pruning_suppressed {
        return GenerationRetentionApplyReport {
            dry_run: false,
            project_id: plan.project_id.clone(),
            pruning_suppressed: true,
            active_bytes: plan.active_bytes,
            rollback_bytes: plan.rollback_bytes,
            building_bytes: plan.building_bytes,
            retained_bytes: plan.retained_bytes,
            reclaimable_bytes: plan.reclaimable_bytes,
            removed_bytes: 0,
            remaining_reclaimable_bytes: plan.reclaimable_bytes,
            removals: Vec::new(),
            errors: plan.errors.clone(),
        };
    }

    let mut removed_bytes = 0_u64;
    let mut removals = Vec::new();
    for bundle in plan
        .bundles
        .iter()
        .filter(|bundle| bundle.state == GenerationRetentionState::Reclaimable)
    {
        let mut result = GenerationRemovalResult {
            generation: bundle.generation.clone(),
            qdrant_collection: bundle.qdrant_collection.clone(),
            removed_paths: Vec::new(),
            qdrant_collection_removed: false,
            removed_bytes: 0,
            remaining_reclaimable_bytes: bundle.bytes,
            errors: Vec::new(),
        };
        match remover.delete_qdrant_collection(&bundle.qdrant_collection) {
            Ok(true) => {
                result.qdrant_collection_removed = true;
                result.removed_bytes = result
                    .removed_bytes
                    .saturating_add(bundle.qdrant.as_ref().map_or(0, |item| item.bytes));
            }
            Ok(false) => result.errors.push(format!(
                "delete Qdrant collection {} was acknowledged but its local data remains",
                bundle.qdrant_collection
            )),
            Err(error) => result.errors.push(format!(
                "delete Qdrant collection {}: {error:#}",
                bundle.qdrant_collection
            )),
        }
        for artifact in [bundle.lexical.as_ref(), bundle.scip.as_ref()]
            .into_iter()
            .flatten()
        {
            match remover.remove_generation_dir(&artifact.path) {
                Ok(()) => {
                    result.removed_paths.push(artifact.path.clone());
                    result.removed_bytes = result.removed_bytes.saturating_add(artifact.bytes);
                }
                Err(error) => result.errors.push(format!(
                    "remove generation path {}: {error:#}",
                    artifact.path.display()
                )),
            }
        }
        result.remaining_reclaimable_bytes = bundle.bytes.saturating_sub(result.removed_bytes);
        removed_bytes = removed_bytes.saturating_add(result.removed_bytes);
        removals.push(result);
    }
    let errors = removals
        .iter()
        .flat_map(|result| result.errors.iter().cloned())
        .collect();
    GenerationRetentionApplyReport {
        dry_run: false,
        project_id: plan.project_id.clone(),
        pruning_suppressed: false,
        active_bytes: plan.active_bytes,
        rollback_bytes: plan.rollback_bytes,
        building_bytes: plan.building_bytes,
        retained_bytes: plan.retained_bytes,
        reclaimable_bytes: plan.reclaimable_bytes,
        removed_bytes,
        remaining_reclaimable_bytes: plan.reclaimable_bytes.saturating_sub(removed_bytes),
        removals,
        errors,
    }
}

#[derive(Debug, Clone, Copy)]
enum ArtifactKind {
    Lexical,
    Scip,
    Qdrant,
}

#[derive(Default)]
struct BundleBuilder {
    lexical: Option<GenerationArtifact>,
    scip: Option<GenerationArtifact>,
    qdrant: Option<GenerationArtifact>,
}

impl BundleBuilder {
    fn set(&mut self, kind: ArtifactKind, artifact: GenerationArtifact) {
        match kind {
            ArtifactKind::Lexical => self.lexical = Some(artifact),
            ArtifactKind::Scip => self.scip = Some(artifact),
            ArtifactKind::Qdrant => self.qdrant = Some(artifact),
        }
    }

    fn finish(
        self,
        project_id: &str,
        generation: String,
        state: GenerationRetentionState,
    ) -> GenerationBundle {
        let bytes = [&self.lexical, &self.scip, &self.qdrant]
            .into_iter()
            .flatten()
            .fold(0_u64, |total, artifact| {
                total.saturating_add(artifact.bytes)
            });
        let suffix = generation
            .strip_prefix(&format!("{project_id}-"))
            .expect("planned generation is canonical")
            .to_string();
        GenerationBundle {
            generation,
            qdrant_collection: format!("codestory_{project_id}_{suffix}"),
            state,
            lexical: self.lexical,
            scip: self.scip,
            qdrant: self.qdrant,
            bytes,
        }
    }
}

fn retention_dir(state_file: &Path) -> PathBuf {
    state_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(RETENTION_DIR)
}

fn ensure_retention_dir(state_file: &Path) -> Result<PathBuf> {
    let path = retention_dir(state_file);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!(
                "retention path is not a direct directory: {}",
                path.display()
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&path)
                .with_context(|| format!("create retention directory {}", path.display()))?;
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("inspect retention directory {}", path.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!(
                    "retention path is not a direct directory: {}",
                    path.display()
                );
            }
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect retention directory {}", path.display()));
        }
    }
    Ok(path)
}

fn validate_retention_component(component: &str) -> Result<()> {
    if component.is_empty()
        || !component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("value is not a safe retention path component");
    }
    Ok(())
}

fn validate_marker(marker: &GenerationRetentionMarker) -> Result<()> {
    if marker.schema_version != RETENTION_SCHEMA_VERSION {
        bail!("unsupported generation retention marker schema");
    }
    validate_retention_component(&marker.workspace_id)?;
    if marker.project_id != marker.active.project_id {
        bail!("generation retention marker active project does not match marker project");
    }
    let active_generation = canonical_manifest_generation(&marker.active)?;
    if let Some(rollback) = marker.rollback.as_ref() {
        if rollback.manifest.project_id != marker.project_id {
            bail!("generation retention rollback project does not match marker project");
        }
        if canonical_manifest_generation(&rollback.manifest)? == active_generation {
            bail!("generation retention rollback duplicates the active generation");
        }
    }
    Ok(())
}

fn read_marker_path(path: &Path) -> Result<Option<GenerationRetentionMarker>> {
    if !path.exists() {
        return Ok(None);
    }
    let marker: GenerationRetentionMarker = serde_json::from_slice(
        &std::fs::read(path)
            .with_context(|| format!("read generation retention marker {}", path.display()))?,
    )
    .with_context(|| format!("parse generation retention marker {}", path.display()))?;
    validate_marker(&marker)?;
    Ok(Some(marker))
}

fn canonical_manifest_generation(manifest: &RetrievalIndexManifest) -> Result<String> {
    let generation = manifest
        .sidecar_generation
        .as_deref()
        .context("retrieval manifest is missing sidecar generation")?;
    let Some(suffix) = canonical_generation_suffix(&manifest.project_id, generation) else {
        bail!("retrieval manifest has a noncanonical sidecar generation");
    };
    if manifest.qdrant_collection != format!("codestory_{}_{suffix}", manifest.project_id) {
        bail!("retrieval manifest generation and Qdrant collection do not match");
    }
    Ok(generation.to_string())
}

fn canonical_generation_suffix<'a>(project_id: &str, generation: &'a str) -> Option<&'a str> {
    canonical_suffix(generation.strip_prefix(&format!("{project_id}-"))?)
}

fn canonical_collection_suffix<'a>(project_id: &str, collection: &'a str) -> Option<&'a str> {
    canonical_suffix(collection.strip_prefix(&format!("codestory_{project_id}_"))?)
}

fn canonical_suffix(suffix: &str) -> Option<&str> {
    (suffix.len() == 16
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then_some(suffix)
}

fn storage_paths_for_scan(
    cache_root: &Path,
    active_storage_path: Option<&Path>,
    errors: &mut Vec<String>,
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    if let Some(path) = active_storage_path {
        insert_direct_storage_path(path, "active storage", &mut paths, errors);
    }
    let flat = cache_root.join("codestory.db");
    insert_direct_storage_path(&flat, "flat cache storage", &mut paths, errors);
    if !cache_root.exists() {
        return paths;
    }
    match std::fs::read_dir(cache_root) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let file_type = match entry.file_type() {
                            Ok(file_type) => file_type,
                            Err(error) => {
                                errors.push(format!(
                                    "read cache entry type {}: {error}",
                                    entry.path().display()
                                ));
                                continue;
                            }
                        };
                        if file_type.is_symlink() {
                            errors.push(format!(
                                "cache scan refuses linked entry {}",
                                entry.path().display()
                            ));
                            continue;
                        }
                        if !file_type.is_dir() {
                            continue;
                        }
                        let path = entry.path().join("codestory.db");
                        insert_direct_storage_path(
                            &path,
                            "project cache storage",
                            &mut paths,
                            errors,
                        );
                    }
                    Err(error) => errors.push(format!(
                        "read cache entry under {}: {error}",
                        cache_root.display()
                    )),
                }
            }
        }
        Err(error) => errors.push(format!(
            "read cache root {} for retention manifests: {error}",
            cache_root.display()
        )),
    }
    paths
}

fn insert_direct_storage_path(
    path: &Path,
    label: &str,
    paths: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<String>,
) {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => errors.push(
            format!("{label} is not a direct regular file: {}", path.display()),
        ),
        Ok(_) => {
            paths.insert(path.to_path_buf());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => errors.push(format!("inspect {label} {}: {error}", path.display())),
    }
}

fn deduplicate_manifests(manifests: &mut Vec<RetrievalIndexManifest>) {
    manifests.sort_by(|left, right| {
        (
            &left.project_id,
            &left.sidecar_generation,
            &left.qdrant_collection,
        )
            .cmp(&(
                &right.project_id,
                &right.sidecar_generation,
                &right.qdrant_collection,
            ))
    });
    manifests.dedup_by(|left, right| {
        left.project_id == right.project_id
            && left.sidecar_generation == right.sidecar_generation
            && left.qdrant_collection == right.qdrant_collection
    });
}

fn collect_protected_generations(
    project_id: &str,
    manifests: &[RetrievalIndexManifest],
    generations: &mut BTreeSet<String>,
    errors: &mut Vec<String>,
    role: &str,
) {
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.project_id == project_id)
    {
        match canonical_manifest_generation(manifest) {
            Ok(generation) => {
                generations.insert(generation);
            }
            Err(error) => errors.push(format!(
                "{role} manifest for {project_id} is not safe retention evidence: {error:#}"
            )),
        }
    }
}

fn discover_generation_dirs(
    root: &Path,
    project_id: &str,
    kind: ArtifactKind,
    builders: &mut BTreeMap<String, BundleBuilder>,
    blocked: &mut Vec<BlockedGenerationEntry>,
    errors: &mut Vec<String>,
) {
    let Some(entries) = read_direct_directory(root, "generation root", errors) else {
        return;
    };
    let project_prefix = format!("{project_id}-");
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!(
                    "read generation entry in {}: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&project_prefix) {
            continue;
        }
        let Some(_) = canonical_generation_suffix(project_id, &name) else {
            block_scoped_entry(path, "malformed generation name", blocked, errors);
            continue;
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                errors.push(format!("read generation type {}: {error}", path.display()));
                continue;
            }
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            block_scoped_entry(
                path,
                "generation entry is not a direct directory",
                blocked,
                errors,
            );
            continue;
        }
        match directory_size(&path) {
            Ok(bytes) => builders
                .entry(name)
                .or_default()
                .set(kind, GenerationArtifact { path, bytes }),
            Err(error) => errors.push(format!("measure generation {}: {error:#}", path.display())),
        }
    }
}

fn discover_qdrant_collections(
    root: &Path,
    project_id: &str,
    builders: &mut BTreeMap<String, BundleBuilder>,
    blocked: &mut Vec<BlockedGenerationEntry>,
    errors: &mut Vec<String>,
) {
    let Some(entries) = read_direct_directory(root, "Qdrant collection root", errors) else {
        return;
    };
    let project_prefix = format!("codestory_{project_id}_");
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!(
                    "read Qdrant collection in {}: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&project_prefix) {
            continue;
        }
        let Some(suffix) = canonical_collection_suffix(project_id, &name) else {
            block_scoped_entry(path, "malformed Qdrant collection name", blocked, errors);
            continue;
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                errors.push(format!(
                    "read Qdrant collection type {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            block_scoped_entry(
                path,
                "Qdrant collection entry is not a direct directory",
                blocked,
                errors,
            );
            continue;
        }
        match directory_size(&path) {
            Ok(bytes) => builders
                .entry(format!("{project_id}-{suffix}"))
                .or_default()
                .set(ArtifactKind::Qdrant, GenerationArtifact { path, bytes }),
            Err(error) => errors.push(format!(
                "measure Qdrant collection {}: {error:#}",
                path.display()
            )),
        }
    }
}

fn discover_live_qdrant_collections(
    collections: &[String],
    project_id: &str,
    builders: &mut BTreeMap<String, BundleBuilder>,
    blocked: &mut Vec<BlockedGenerationEntry>,
    errors: &mut Vec<String>,
) {
    let project_prefix = format!("codestory_{project_id}_");
    for collection in collections {
        if !collection.starts_with(&project_prefix) {
            continue;
        }
        let Some(suffix) = canonical_collection_suffix(project_id, collection) else {
            block_scoped_entry(
                PathBuf::from(format!("qdrant:{collection}")),
                "malformed live Qdrant collection name",
                blocked,
                errors,
            );
            continue;
        };
        builders
            .entry(format!("{project_id}-{suffix}"))
            .or_default();
    }
}

fn block_scoped_entry(
    path: PathBuf,
    reason: &str,
    blocked: &mut Vec<BlockedGenerationEntry>,
    errors: &mut Vec<String>,
) {
    errors.push(format!(
        "scoped retention entry {} is unsafe: {reason}",
        path.display()
    ));
    blocked.push(BlockedGenerationEntry {
        path,
        reason: reason.to_string(),
    });
}

fn read_direct_directory(
    root: &Path,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<std::fs::ReadDir> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            errors.push(format!(
                "{label} is not a direct directory: {}",
                root.display()
            ));
            None
        }
        Ok(_) => match std::fs::read_dir(root) {
            Ok(entries) => Some(entries),
            Err(error) => {
                errors.push(format!("read {label} {}: {error}", root.display()));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            errors.push(format!("inspect {label} {}: {error}", root.display()));
            None
        }
    }
}

fn direct_directory_exists_or_missing(root: &Path, label: &str, errors: &mut Vec<String>) -> bool {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            errors.push(format!(
                "{label} is not a direct directory: {}",
                root.display()
            ));
            false
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            errors.push(format!("inspect {label} {}: {error}", root.display()));
            false
        }
    }
}

fn directory_size(path: &Path) -> Result<u64> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("path is not a direct directory");
    }
    let mut total = 0_u64;
    for entry in
        std::fs::read_dir(path).with_context(|| format!("read directory {}", path.display()))?
    {
        let entry = entry.with_context(|| format!("read entry under {}", path.display()))?;
        let entry_path = entry.path();
        let metadata = std::fs::symlink_metadata(&entry_path)
            .with_context(|| format!("inspect {}", entry_path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!("directory contains a link: {}", entry_path.display());
        }
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry_path)?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else {
            bail!(
                "directory contains an unsupported entry: {}",
                entry_path.display()
            );
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn manifest(project_id: &str, suffix: &str, built_at_epoch_ms: i64) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: project_id.into(),
            lexical_version: "v1".into(),
            qdrant_collection: format!("codestory_{project_id}_{suffix}"),
            scip_revision: Some(format!("graph-{suffix}")),
            built_at_epoch_ms,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some("llamacpp:bge-base-en-v1.5".into()),
            embedding_dim: Some(768),
            sidecar_schema_version: Some(2),
            sidecar_input_hash: Some(suffix.repeat(4)),
            sidecar_generation: Some(format!("{project_id}-{suffix}")),
            projection_count: Some(1),
            symbol_doc_count: Some(1),
            dense_projection_count: Some(1),
            semantic_policy_version: Some("graph_first_v1".into()),
            graph_artifact_hash: Some("graph".into()),
            dense_reason_counts_json: Some("{}".into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        }
    }

    fn layout(root: &Path) -> SidecarLayout {
        SidecarLayout {
            qdrant_http_port: 9,
            qdrant_grpc_port: 10,
            lexical_data_dir: root.join("lexical"),
            qdrant_data_dir: root.join("qdrant"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("retrieval-sidecars.json"),
        }
    }

    fn write_bundle(layout: &SidecarLayout, project_id: &str, suffix: &str, sizes: [usize; 3]) {
        let generation = format!("{project_id}-{suffix}");
        let lexical = layout.lexical_data_dir.join("shards").join(&generation);
        let scip = layout.scip_artifacts_root.join(&generation);
        let qdrant = layout
            .qdrant_data_dir
            .join("collections")
            .join(format!("codestory_{project_id}_{suffix}"));
        for (dir, size) in [(&lexical, sizes[0]), (&scip, sizes[1]), (&qdrant, sizes[2])] {
            std::fs::create_dir_all(dir).expect("artifact dir");
            std::fs::write(dir.join("data"), vec![b'x'; size]).expect("artifact bytes");
        }
    }

    #[test]
    fn local_and_agent_profiles_share_one_global_gc_coordination_file() {
        let root = tempdir().expect("root");
        let runtime = |profile, state_file| SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                state_file,
                ..layout(root.path())
            },
            profile,
            run_id: None,
            namespace: "test".into(),
            compose_project: "test".into(),
            embed_http_port: 0,
            cleanup_command: String::new(),
            labels: BTreeMap::new(),
            ..SidecarRuntimeConfig::local()
        };
        let local = runtime(
            SidecarProfile::Local,
            root.path().join("retrieval-sidecars.json"),
        );
        let agent = runtime(
            SidecarProfile::Agent,
            root.path()
                .join("sidecars")
                .join("codestory-agent-test")
                .join("retrieval-sidecars.json"),
        );

        assert_eq!(
            global_generation_gc_state_file(&local),
            global_generation_gc_state_file(&agent)
        );
        assert_eq!(
            global_generation_gc_state_file(&local),
            root.path().join("generation-retention-coordination.state")
        );
    }

    #[derive(Default)]
    struct TestRemover {
        removed_paths: Vec<PathBuf>,
        removed_collections: Vec<String>,
        fail_path_fragment: Option<String>,
        qdrant_data_remains: bool,
    }

    impl GenerationRemover for TestRemover {
        fn remove_generation_dir(&mut self, path: &Path) -> Result<()> {
            if self
                .fail_path_fragment
                .as_deref()
                .is_some_and(|fragment| path.display().to_string().contains(fragment))
            {
                bail!("planned path failure");
            }
            std::fs::remove_dir_all(path)?;
            self.removed_paths.push(path.to_path_buf());
            Ok(())
        }

        fn delete_qdrant_collection(&mut self, collection: &str) -> Result<bool> {
            self.removed_collections.push(collection.to_string());
            Ok(!self.qdrant_data_remains)
        }
    }

    #[test]
    fn active_and_rollback_are_retained_while_stale_bundle_is_removed() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let active = "aaaaaaaaaaaaaaaa";
        let rollback = "bbbbbbbbbbbbbbbb";
        let stale = "cccccccccccccccc";
        write_bundle(&layout, project, active, [1, 2, 3]);
        write_bundle(&layout, project, rollback, [4, 5, 6]);
        write_bundle(&layout, project, stale, [7, 8, 9]);
        let protection = RetentionProtectionScan {
            authoritative_active: vec![manifest(project, active, 3)],
            rollback: vec![manifest(project, rollback, 2)],
            ..RetentionProtectionScan::default()
        };

        let plan = plan_generation_retention(&layout, project, &protection);

        assert!(!plan.pruning_suppressed);
        assert_eq!(plan.active_bytes, 6);
        assert_eq!(plan.rollback_bytes, 15);
        assert_eq!(plan.building_bytes, 0);
        assert_eq!(plan.retained_bytes, 21);
        assert_eq!(plan.reclaimable_bytes, 24);
        assert_eq!(
            plan.bundles
                .iter()
                .filter(|bundle| bundle.state == GenerationRetentionState::Reclaimable)
                .map(|bundle| bundle.generation.as_str())
                .collect::<Vec<_>>(),
            vec!["repo-v1-project-cccccccccccccccc"]
        );

        let mut remover = TestRemover::default();
        let report = apply_generation_retention(&plan, &mut remover);

        assert_eq!(report.removed_bytes, 24);
        assert_eq!(report.remaining_reclaimable_bytes, 0);
        assert_eq!(remover.removed_paths.len(), 2);
        assert_eq!(
            remover.removed_collections,
            vec!["codestory_repo-v1-project_cccccccccccccccc"]
        );
        assert!(
            layout
                .lexical_data_dir
                .join("shards")
                .join(format!("{project}-{active}"))
                .is_dir()
        );
    }

    #[test]
    fn every_shared_active_and_rollback_manifest_is_a_gc_root() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        for suffix in [
            "aaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbb",
            "cccccccccccccccc",
            "dddddddddddddddd",
            "eeeeeeeeeeeeeeee",
            "ffffffffffffffff",
        ] {
            write_bundle(&layout, project, suffix, [1, 1, 1]);
        }
        let state_file = root.path().join("retrieval-sidecars.json");
        for (workspace, active, rollback) in [
            ("workspace_a", "aaaaaaaaaaaaaaaa", "dddddddddddddddd"),
            ("workspace_b", "bbbbbbbbbbbbbbbb", "eeeeeeeeeeeeeeee"),
        ] {
            let marker = GenerationRetentionMarker::next(
                workspace,
                manifest(project, active, 10),
                Some(VerifiedRollbackManifest {
                    manifest: manifest(project, rollback, 5),
                    verified_at_epoch_ms: 10,
                }),
                10,
            )
            .expect("marker");
            write_retention_marker(&state_file, &marker).expect("write marker");
        }
        let mut protection = scan_retention_protection(root.path(), None, &state_file);
        protection
            .authoritative_active
            .push(manifest(project, "cccccccccccccccc", 30));

        let plan = plan_generation_retention(&layout, project, &protection);

        assert_eq!(
            plan.bundles
                .iter()
                .filter(|bundle| bundle.state != GenerationRetentionState::Reclaimable)
                .map(|bundle| (bundle.generation.clone(), bundle.state))
                .collect::<Vec<_>>(),
            vec![
                (
                    "repo-v1-project-aaaaaaaaaaaaaaaa".to_string(),
                    GenerationRetentionState::Active,
                ),
                (
                    "repo-v1-project-bbbbbbbbbbbbbbbb".to_string(),
                    GenerationRetentionState::Active,
                ),
                (
                    "repo-v1-project-cccccccccccccccc".to_string(),
                    GenerationRetentionState::Active,
                ),
                (
                    "repo-v1-project-dddddddddddddddd".to_string(),
                    GenerationRetentionState::Rollback,
                ),
                (
                    "repo-v1-project-eeeeeeeeeeeeeeee".to_string(),
                    GenerationRetentionState::Rollback,
                ),
            ]
        );
        assert_eq!(
            plan.bundles
                .iter()
                .filter(|bundle| bundle.state == GenerationRetentionState::Reclaimable)
                .map(|bundle| bundle.generation.as_str())
                .collect::<Vec<_>>(),
            vec!["repo-v1-project-ffffffffffffffff"]
        );
    }

    #[test]
    fn unrooted_bytes_are_building_until_the_retention_view_is_stable() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        write_bundle(&layout, project, "aaaaaaaaaaaaaaaa", [1, 2, 3]);
        write_bundle(&layout, project, "bbbbbbbbbbbbbbbb", [4, 5, 6]);
        let protection = RetentionProtectionScan {
            authoritative_active: vec![manifest(project, "aaaaaaaaaaaaaaaa", 1)],
            ..RetentionProtectionScan::default()
        };

        let plan = plan_generation_retention_with_unrooted_state(
            &layout,
            project,
            &protection,
            &[],
            GenerationRetentionState::Building,
        );

        assert_eq!(plan.active_bytes, 6);
        assert_eq!(plan.rollback_bytes, 0);
        assert_eq!(plan.building_bytes, 15);
        assert_eq!(plan.reclaimable_bytes, 0);
        assert_eq!(plan.retained_bytes, 21);
        assert!(plan.pruning_suppressed);
    }

    #[test]
    fn malformed_and_non_directory_entries_are_blocked_not_candidates() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let shards = layout.lexical_data_dir.join("shards");
        std::fs::create_dir_all(shards.join(format!("{project}-not-hex"))).expect("malformed");
        std::fs::write(
            shards.join(format!("{project}-dddddddddddddddd")),
            "not a directory",
        )
        .expect("file");

        let plan = plan_generation_retention(&layout, project, &RetentionProtectionScan::default());

        assert_eq!(plan.blocked.len(), 2);
        assert!(plan.bundles.is_empty());
        assert_eq!(plan.reclaimable_bytes, 0);
        assert!(plan.pruning_suppressed);
    }

    #[test]
    fn scoped_malformed_entry_suppresses_otherwise_valid_stale_deletion() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let stale = "cccccccccccccccc";
        write_bundle(&layout, project, stale, [2, 3, 4]);
        std::fs::create_dir_all(
            layout
                .scip_artifacts_root
                .join(format!("{project}-malformed")),
        )
        .expect("malformed");

        let plan = plan_generation_retention(&layout, project, &RetentionProtectionScan::default());
        let mut remover = TestRemover::default();
        let report = apply_generation_retention(&plan, &mut remover);

        assert!(plan.pruning_suppressed);
        assert_eq!(plan.building_bytes, 9);
        assert_eq!(plan.reclaimable_bytes, 0);
        assert!(report.pruning_suppressed);
        assert_eq!(report.removed_bytes, 0);
        assert!(remover.removed_paths.is_empty());
        assert!(remover.removed_collections.is_empty());
        assert!(
            layout
                .lexical_data_dir
                .join("shards")
                .join(format!("{project}-{stale}"))
                .is_dir()
        );
    }

    #[test]
    fn missing_active_generation_suppresses_stale_deletion() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        write_bundle(&layout, project, "cccccccccccccccc", [2, 3, 4]);

        let plan = plan_generation_retention(&layout, project, &RetentionProtectionScan::default());
        let mut remover = TestRemover::default();
        let report = apply_generation_retention(&plan, &mut remover);

        assert!(plan.pruning_suppressed);
        assert_eq!(plan.building_bytes, 9);
        assert_eq!(plan.reclaimable_bytes, 0);
        assert!(report.pruning_suppressed);
        assert!(remover.removed_paths.is_empty());
        assert!(remover.removed_collections.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_generation_is_blocked_without_following_target() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("root");
        let outside = tempdir().expect("outside");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let shards = layout.lexical_data_dir.join("shards");
        std::fs::create_dir_all(&shards).expect("shards");
        std::fs::write(outside.path().join("keep"), "outside").expect("outside file");
        symlink(
            outside.path(),
            shards.join(format!("{project}-eeeeeeeeeeeeeeee")),
        )
        .expect("symlink");

        let plan = plan_generation_retention(&layout, project, &RetentionProtectionScan::default());

        assert_eq!(plan.blocked.len(), 1);
        assert!(plan.bundles.is_empty());
        assert!(plan.pruning_suppressed);
        assert!(outside.path().join("keep").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_generation_root_suppresses_pruning_without_following_target() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("root");
        let outside = tempdir().expect("outside");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let generation = format!("{project}-cccccccccccccccc");
        std::fs::create_dir_all(outside.path().join(&generation)).expect("outside generation");
        std::fs::create_dir_all(&layout.lexical_data_dir).expect("lexical parent");
        symlink(outside.path(), layout.lexical_data_dir.join("shards")).expect("linked root");
        let protection = RetentionProtectionScan {
            authoritative_active: vec![manifest(project, "aaaaaaaaaaaaaaaa", 1)],
            ..RetentionProtectionScan::default()
        };

        let plan = plan_generation_retention(&layout, project, &protection);

        assert!(plan.pruning_suppressed);
        assert!(
            plan.errors
                .iter()
                .any(|error| error.contains("generation root is not a direct directory"))
        );
        assert!(outside.path().join(generation).is_dir());
    }

    #[test]
    fn deletion_failure_is_reported_without_touching_active_generation() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let active = "aaaaaaaaaaaaaaaa";
        let stale = "cccccccccccccccc";
        write_bundle(&layout, project, active, [1, 1, 1]);
        write_bundle(&layout, project, stale, [2, 3, 4]);
        let plan = plan_generation_retention(
            &layout,
            project,
            &RetentionProtectionScan {
                authoritative_active: vec![manifest(project, active, 2)],
                ..RetentionProtectionScan::default()
            },
        );
        let mut remover = TestRemover {
            fail_path_fragment: Some("scip".into()),
            ..TestRemover::default()
        };

        let report = apply_generation_retention(&plan, &mut remover);

        assert_eq!(report.removed_bytes, 6);
        assert_eq!(report.remaining_reclaimable_bytes, 3);
        assert_eq!(report.errors.len(), 1);
        assert!(
            layout
                .scip_artifacts_root
                .join(format!("{project}-{stale}"))
                .is_dir()
        );
        assert!(
            layout
                .scip_artifacts_root
                .join(format!("{project}-{active}"))
                .is_dir()
        );
    }

    #[test]
    fn acknowledged_qdrant_delete_does_not_overstate_removed_bytes_when_data_remains() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let active = "aaaaaaaaaaaaaaaa";
        let stale = "cccccccccccccccc";
        write_bundle(&layout, project, active, [1, 1, 1]);
        write_bundle(&layout, project, stale, [2, 3, 4]);
        let plan = plan_generation_retention(
            &layout,
            project,
            &RetentionProtectionScan {
                authoritative_active: vec![manifest(project, active, 2)],
                ..RetentionProtectionScan::default()
            },
        );
        let mut remover = TestRemover {
            qdrant_data_remains: true,
            ..TestRemover::default()
        };

        let report = apply_generation_retention(&plan, &mut remover);

        assert_eq!(report.removed_bytes, 5);
        assert_eq!(report.remaining_reclaimable_bytes, 4);
        assert!(!report.removals[0].qdrant_collection_removed);
        assert!(report.errors[0].contains("local data remains"));
    }

    #[test]
    fn live_qdrant_collection_is_planned_when_disk_directory_is_absent() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let active = "aaaaaaaaaaaaaaaa";
        let collection = "codestory_repo-v1-project_ffffffffffffffff".to_string();
        let protection = RetentionProtectionScan {
            authoritative_active: vec![manifest(project, active, 1)],
            ..RetentionProtectionScan::default()
        };

        let plan = plan_generation_retention_with_qdrant_collections(
            &layout,
            project,
            &protection,
            std::slice::from_ref(&collection),
        );

        assert!(!plan.pruning_suppressed);
        let stale = plan
            .bundles
            .iter()
            .find(|bundle| bundle.qdrant_collection == collection)
            .expect("live-only stale collection");
        assert_eq!(stale.state, GenerationRetentionState::Reclaimable);
        assert!(stale.qdrant.is_none());
        let mut remover = TestRemover::default();
        let report = apply_generation_retention(&plan, &mut remover);
        assert_eq!(remover.removed_collections, vec![collection]);
        assert_eq!(report.removed_bytes, 0);
    }

    #[test]
    fn marker_update_preserves_only_a_freshly_verified_rollback() {
        let project = "repo-v1-project";
        let active = manifest(project, "aaaaaaaaaaaaaaaa", 10);
        let rollback = VerifiedRollbackManifest {
            manifest: manifest(project, "bbbbbbbbbbbbbbbb", 9),
            verified_at_epoch_ms: 11,
        };
        let mut refreshed_active = active;
        refreshed_active.built_at_epoch_ms = 20;

        let refreshed = GenerationRetentionMarker::next(
            "workspace_1",
            refreshed_active,
            Some(rollback.clone()),
            21,
        )
        .expect("refresh marker");

        assert_eq!(refreshed.rollback, Some(rollback));
    }

    #[test]
    fn marker_write_is_atomic_and_protection_scan_reports_malformed_marker() {
        let root = tempdir().expect("root");
        let state_file = root.path().join("retrieval-sidecars.json");
        let marker = GenerationRetentionMarker::next(
            "workspace_1",
            manifest("repo-v1-project", "aaaaaaaaaaaaaaaa", 1),
            None,
            2,
        )
        .expect("marker");
        write_retention_marker(&state_file, &marker).expect("write marker");
        assert_eq!(
            read_retention_marker(&state_file, "workspace_1").expect("read marker"),
            Some(marker)
        );
        std::fs::write(retention_dir(&state_file).join("bad.json"), "{").expect("bad marker");

        let scan = scan_retention_protection(root.path(), None, &state_file);

        assert_eq!(scan.active.len(), 1);
        assert_eq!(scan.errors.len(), 1);
        assert!(scan.errors[0].contains("bad.json"));
    }
}
