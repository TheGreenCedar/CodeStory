//! Bounded, post-publication retention for one sidecar namespace.

use crate::config::{SidecarLayout, SidecarProfile, SidecarRuntimeConfig};
use anyhow::{Context, Result, bail};
use codestory_contracts::bounded_locks::{
    self, FileLockKind, LockDeadline, PUBLICATION_LOCK_WAIT, acquire_with_deadline,
};
use codestory_store::{RetrievalIndexManifest, RetrievalIndexRollbackRecord, Store};
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Marker schema 1: no workspace registration, so its roots can never be
/// proven retired. Still read so a peer running an older binary keeps pinning
/// the generations it is serving.
pub const RETENTION_MARKER_SCHEMA_V1: u32 = 1;
/// Marker schema 2 adds the workspace registration (`owner`), the canonical
/// generation it pins, and a heartbeat, which is what lets a dead worktree be
/// retired instead of pinning its generations forever.
pub const RETENTION_MARKER_SCHEMA_V2: u32 = 2;
/// The schema this binary writes. Readers accept every schema in
/// [`RETENTION_MARKER_SCHEMA_V1`]..=[`RETENTION_MARKER_SCHEMA_V2`].
const RETENTION_SCHEMA_VERSION: u32 = RETENTION_MARKER_SCHEMA_V2;
const RETENTION_DIR: &str = "retention";
/// Extensions the retention directory is allowed to contain. Anything else is
/// evidence this reader does not understand, so it protects instead of being
/// skipped: a future marker encoding must not silently read as "no roots".
const RETENTION_MARKER_EXTENSION: &str = "json";
const RETENTION_LOCK_EXTENSION: &str = "lock";
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

/// Workspace registration carried by a schema-2 marker.
///
/// `project_root` is the registration itself: without it a marker's roots can
/// never be proven retired, which is why a schema-1 marker stays protected
/// forever. `workspace_id` is repeated here so a registration copied into the
/// wrong marker file fails validation rather than retiring another workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionMarkerOwner {
    pub workspace_id: String,
    pub project_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRetentionMarker {
    pub schema_version: u32,
    pub workspace_id: String,
    pub project_id: String,
    pub active: RetrievalIndexManifest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback: Option<RetrievalIndexRollbackRecord>,
    pub updated_at_epoch_ms: i64,
    /// Schema 2 only. Absent in a schema-1 marker, which dual-read still
    /// accepts as an unretirable root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<RetentionMarkerOwner>,
    /// Schema 2 only: the canonical generation this marker pins, recorded so a
    /// reader can report the pin without re-deriving it from the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    /// Schema 2 only: last time the registering workspace refreshed this
    /// marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_epoch_ms: Option<i64>,
}

impl GenerationRetentionMarker {
    pub fn next(
        workspace_id: &str,
        project_root: &Path,
        active: RetrievalIndexManifest,
        verified_previous: Option<RetrievalIndexRollbackRecord>,
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
        let project_root = project_root
            .to_str()
            .context("workspace registration requires a UTF-8 project root")?
            .to_string();
        let marker = Self {
            schema_version: RETENTION_SCHEMA_VERSION,
            workspace_id: workspace_id.to_string(),
            project_id: active.project_id.clone(),
            active,
            rollback,
            updated_at_epoch_ms,
            owner: Some(RetentionMarkerOwner {
                workspace_id: workspace_id.to_string(),
                project_root,
            }),
            generation: Some(active_generation),
            heartbeat_epoch_ms: Some(updated_at_epoch_ms),
        };
        validate_marker(&marker)?;
        Ok(marker)
    }

    /// Whether this marker's roots may be retired without further evidence.
    ///
    /// Only a schema-2 registration whose recorded project root has since
    /// disappeared qualifies. Every other shape — schema 1, an unreadable
    /// root, or a root that still exists — keeps protecting.
    pub fn retirement(&self) -> MarkerRetirement {
        let Some(owner) = self.owner.as_ref() else {
            return MarkerRetirement::UnregisteredWorkspace;
        };
        let project_root = Path::new(&owner.project_root);
        match std::fs::symlink_metadata(project_root) {
            Ok(_) => return MarkerRetirement::LiveWorkspace,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return MarkerRetirement::UnprovenWorkspace,
        }
        // A missing leaf under a directory that is still there is a deleted
        // worktree. A missing leaf whose parent is also gone is indistinguishable
        // from an unplugged drive or an unmounted network share, and an absent
        // volume is not evidence that the workspace was abandoned.
        let Some(parent) = project_root
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return MarkerRetirement::UnprovenWorkspace;
        };
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.is_dir() => MarkerRetirement::RetiredWorkspace,
            Ok(_) | Err(_) => MarkerRetirement::UnprovenWorkspace,
        }
    }
}

/// Why one marker's roots do or do not still protect their generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerRetirement {
    /// Schema-1 marker: no registration exists, so retirement is unprovable.
    UnregisteredWorkspace,
    /// The registered project root still exists.
    LiveWorkspace,
    /// The registered project root could not be observed at all.
    UnprovenWorkspace,
    /// The registered project root is provably gone.
    RetiredWorkspace,
}

impl MarkerRetirement {
    /// A marker stops rooting its generations only under proven retirement.
    pub const fn still_protects(self) -> bool {
        !matches!(self, Self::RetiredWorkspace)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnregisteredWorkspace => "unregistered_workspace",
            Self::LiveWorkspace => "live_workspace",
            Self::UnprovenWorkspace => "unproven_workspace",
            Self::RetiredWorkspace => "retired_workspace",
        }
    }
}

#[derive(Debug)]
pub struct GenerationRetentionLock {
    file: File,
}

impl GenerationRetentionLock {
    /// Exclusive retention lock for a publication or cleanup pass. The holder
    /// keeps it for the whole pass, so waiters carry the publication budget:
    /// a shorter one would refuse ordinary contention.
    pub fn acquire(state_file: &Path, scope_id: &str) -> Result<Self> {
        Self::acquire_with_cancel(state_file, scope_id, None)
    }

    /// [`Self::acquire`] for a caller that holds the cancellation flag of the
    /// work being done, so the wait ends on cancellation rather than on the
    /// peer's whole publication.
    pub fn acquire_with_cancel(
        state_file: &Path,
        scope_id: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self> {
        Self::acquire_bounded(
            state_file,
            scope_id,
            FileLockKind::Exclusive,
            LockDeadline::after(PUBLICATION_LOCK_WAIT),
            cancel,
        )
    }

    /// The shared side waits behind the same exclusive publication holder, so
    /// it carries the same budget. Ten seconds refused a legitimate commit.
    pub fn acquire_shared(state_file: &Path, scope_id: &str) -> Result<Self> {
        Self::acquire_shared_with_cancel(state_file, scope_id, None)
    }

    /// [`Self::acquire_shared`] for a caller that holds a cancellation flag.
    pub fn acquire_shared_with_cancel(
        state_file: &Path,
        scope_id: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self> {
        Self::acquire_bounded(
            state_file,
            scope_id,
            FileLockKind::Shared,
            LockDeadline::after(PUBLICATION_LOCK_WAIT),
            cancel,
        )
    }

    pub fn try_acquire_shared(state_file: &Path, scope_id: &str) -> Result<Option<Self>> {
        let (path, file) = Self::open_lock_file(state_file, scope_id)?;
        match bounded_locks::try_acquire(&file, FileLockKind::Shared) {
            Ok(true) => Ok(Some(Self { file })),
            Ok(false) => Ok(None),
            Err(error) => Err(anyhow::Error::new(error)).with_context(|| {
                format!("try lock shared generation retention {}", path.display())
            }),
        }
    }

    /// Observe the retention lock without creating it.
    ///
    /// A read-only pass may not be the reason a retention directory or lock
    /// file first appears: an inventory that reports what exists must leave the
    /// cache tree exactly as it found it. A lock file that does not exist
    /// cannot be held by anyone, which is [`ObservedRetentionLock::Absent`].
    pub fn observe_shared(state_file: &Path, scope_id: &str) -> Result<ObservedRetentionLock> {
        let path = retention_lock_path(state_file, scope_id)?;
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ObservedRetentionLock::Absent);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("observe generation retention lock {}", path.display())
                });
            }
        };
        match bounded_locks::try_acquire(&file, FileLockKind::Shared) {
            Ok(true) => Ok(ObservedRetentionLock::Acquired(Self { file })),
            Ok(false) => Ok(ObservedRetentionLock::Contended),
            Err(error) => Err(anyhow::Error::new(error)).with_context(|| {
                format!("try lock shared generation retention {}", path.display())
            }),
        }
    }

    /// Every blocking retention acquisition goes through one absolute deadline:
    /// a sibling publication holding this lock must never be able to stall an
    /// unrelated query, eviction, or shutdown for longer than the caller's
    /// budget.
    pub fn acquire_bounded(
        state_file: &Path,
        scope_id: &str,
        kind: FileLockKind,
        deadline: LockDeadline,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self> {
        let (path, file) = Self::open_lock_file(state_file, scope_id)?;
        acquire_with_deadline(&file, kind, deadline, cancel).map_err(|error| {
            anyhow::Error::new(error).context(format!(
                "acquire {kind} generation retention {}",
                path.display()
            ))
        })?;
        Ok(Self { file })
    }

    fn open_lock_file(state_file: &Path, scope_id: &str) -> Result<(PathBuf, File)> {
        let path = retention_lock_path(state_file, scope_id)?;
        ensure_retention_dir(state_file)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open generation retention lock {}", path.display()))?;
        Ok((path, file))
    }
}

impl Drop for GenerationRetentionLock {
    fn drop(&mut self) {
        let _ = bounded_locks::release(&self.file);
    }
}

/// What a non-creating observation of a retention lock found.
#[derive(Debug)]
pub enum ObservedRetentionLock {
    /// No lock file exists, so no publication can be holding one.
    Absent,
    /// Held shared for as long as this value lives.
    Acquired(GenerationRetentionLock),
    /// A peer holds it exclusively right now.
    Contended,
}

impl ObservedRetentionLock {
    /// Whether the observation proves no exclusive holder was present.
    pub const fn is_quiescent(&self) -> bool {
        matches!(self, Self::Absent | Self::Acquired(_))
    }
}

/// Shared generation locks held for the complete lifetime of one retrieval query session.
///
/// The global lock is always acquired before the project lock. Keeping that order here avoids
/// making every query caller reproduce the publication/GC lock protocol.
pub(crate) struct GenerationRetentionLease {
    _global: GenerationRetentionLock,
    _project: GenerationRetentionLock,
}

impl GenerationRetentionLease {
    pub(crate) fn acquire_for_query(
        runtime: &SidecarRuntimeConfig,
        project_id: &str,
    ) -> Result<Self> {
        let global = GenerationRetentionLock::acquire_shared(
            &global_generation_gc_state_file(runtime),
            GLOBAL_GENERATION_GC_LOCK_SCOPE,
        )
        .context("pin global retrieval generation retention")?;
        let project =
            GenerationRetentionLock::acquire_shared(&runtime.layout.state_file, project_id)
                .with_context(|| format!("pin retrieval generation for project {project_id}"))?;
        Ok(Self {
            _global: global,
            _project: project,
        })
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

#[cfg(test)]
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
    /// Markers whose registered workspace is provably gone, so their roots
    /// were deliberately not collected.
    #[serde(default)]
    pub retired_marker_paths: Vec<PathBuf>,
    /// Set whenever some protection evidence could not be interpreted: an
    /// unreadable or unknown-schema marker, an unrecognized entry in the
    /// retention directory, a marker directory that could not be enumerated,
    /// or a store this binary refuses to observe. It is the typed reason to
    /// suppress pruning, so protection never depends on a caller happening to
    /// treat a free-text error as fatal.
    #[serde(default)]
    pub protection_incomplete: bool,
    pub errors: Vec<String>,
}

impl RetentionProtectionScan {
    fn record_incomplete(&mut self, message: String) {
        self.protection_incomplete = true;
        self.errors.push(message);
    }
}

pub fn scan_retention_protection(
    cache_root: &Path,
    active_storage_path: Option<&Path>,
    state_file: &Path,
) -> RetentionProtectionScan {
    let mut scan = RetentionProtectionScan::default();
    let storage_paths = storage_paths_for_scan(cache_root, active_storage_path, &mut scan);
    for storage_path in storage_paths {
        // Observation only. The live open path recovers interrupted
        // promotions, converts the journal to WAL, and runs the migration
        // ladder, so a scan of another project's cache would rewrite a
        // database this process does not own. An observer that refuses is
        // reported as unreadable protection evidence instead.
        match Store::open_observational(&storage_path)
            .and_then(|store| store.list_retrieval_index_publications())
        {
            Ok(publications) => {
                let manifests = publications
                    .iter()
                    .map(|(manifest, _)| manifest.clone())
                    .collect::<Vec<_>>();
                let rollbacks = publications
                    .into_iter()
                    .filter_map(|(_, rollback)| rollback.map(|record| record.manifest))
                    .collect::<Vec<_>>();
                if active_storage_path.is_some_and(|active| active == storage_path) {
                    scan.authoritative_active.extend(manifests.clone());
                    scan.authoritative_rollback.extend(rollbacks.clone());
                }
                scan.storage_paths_scanned.push(storage_path);
                scan.active.extend(manifests);
                scan.rollback.extend(rollbacks);
            }
            Err(error) => scan.record_incomplete(format!(
                "observe retrieval manifests in {}: {error}",
                storage_path.display()
            )),
        }
    }

    let marker_dir = retention_dir(state_file);
    match std::fs::symlink_metadata(&marker_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            scan.record_incomplete(format!(
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
                            scan.record_incomplete(format!(
                                "read retention marker entry in {}: {error}",
                                marker_dir.display()
                            ));
                            continue;
                        }
                    };
                    let path = entry.path();
                    match classify_retention_entry(&path) {
                        RetentionDirEntry::Marker => {}
                        RetentionDirEntry::Ignorable => continue,
                        RetentionDirEntry::Unrecognized => {
                            scan.record_incomplete(format!(
                                "retention directory holds evidence this reader cannot interpret: {}",
                                path.display()
                            ));
                            continue;
                        }
                    }
                    let file_type = match entry.file_type() {
                        Ok(file_type) => file_type,
                        Err(error) => {
                            scan.record_incomplete(format!(
                                "read retention marker type {}: {error}",
                                path.display()
                            ));
                            continue;
                        }
                    };
                    if file_type.is_symlink() || !file_type.is_file() {
                        scan.record_incomplete(format!(
                            "retention marker is not a direct regular file: {}",
                            path.display()
                        ));
                        continue;
                    }
                    match read_marker_path(&path) {
                        Ok(Some(marker)) => {
                            let retirement = marker.retirement();
                            if !retirement.still_protects() {
                                scan.retired_marker_paths.push(path);
                                continue;
                            }
                            scan.marker_paths_scanned.push(path);
                            scan.active.push(marker.active);
                            if let Some(rollback) = marker.rollback {
                                scan.rollback.push(rollback.manifest);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => scan.record_incomplete(format!(
                            "scan retention marker {}: {error:#}",
                            path.display()
                        )),
                    }
                }
            }
            Err(error) => scan.record_incomplete(format!(
                "read retention marker directory {}: {error}",
                marker_dir.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => scan.record_incomplete(format!(
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
    pub semantic_generation: String,
    pub state: GenerationRetentionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical: Option<GenerationArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scip: Option<GenerationArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "semantic")]
    pub semantic: Option<GenerationArtifact>,
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

pub fn plan_generation_retention(
    layout: &SidecarLayout,
    project_id: &str,
    protection: &RetentionProtectionScan,
) -> GenerationRetentionPlan {
    plan_generation_retention_with_unrooted_state(
        layout,
        project_id,
        protection,
        GenerationRetentionState::Reclaimable,
    )
}

pub(crate) fn plan_generation_retention_with_unrooted_state(
    layout: &SidecarLayout,
    project_id: &str,
    protection: &RetentionProtectionScan,
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
    if direct_directory_exists_or_missing(
        &layout.semantic_data_dir,
        "semantic vector root",
        &mut errors,
    ) {
        discover_vector_generations(
            &layout.semantic_data_dir.join("collections"),
            project_id,
            &mut builders,
            &mut blocked,
            &mut errors,
        );
    }
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

    // Unreadable protection evidence is its own reason to stop, independent of
    // the free-text error list: a marker this reader cannot interpret may be
    // rooting the very generation about to be reclaimed.
    let effective_unrooted_state = if errors.is_empty() && !protection.protection_incomplete {
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
    fn remove_vector_generation(&mut self, generation: &str) -> Result<bool>;
}

pub struct FsGenerationRemover {
    root: PathBuf,
    deletion: OwnedDeletionRoot,
    collections_dir: PathBuf,
}

impl FsGenerationRemover {
    pub fn new(layout: &SidecarLayout) -> Result<Self> {
        let root = layout
            .lexical_data_dir
            .parent()
            .context("lexical data directory has no sidecar root")?;
        if layout.semantic_data_dir.parent() != Some(root)
            || layout.scip_artifacts_root.parent() != Some(root)
        {
            bail!("retrieval generation roots do not share one owned sidecar root");
        }
        let deletion = OwnedDeletionRoot::open(root)
            .with_context(|| format!("open owned sidecar root {}", root.display()))?;
        Ok(Self {
            root: root.to_path_buf(),
            deletion,
            collections_dir: layout.semantic_data_dir.join("collections"),
        })
    }

    fn remove_owned_path(&self, path: &Path) -> Result<bool> {
        let relative = path.strip_prefix(&self.root).with_context(|| {
            format!(
                "generation path {} is outside owned sidecar root {}",
                path.display(),
                self.root.display()
            )
        })?;
        self.deletion
            .remove(relative)
            .with_context(|| format!("remove owned generation path {}", path.display()))
    }
}

impl GenerationRemover for FsGenerationRemover {
    fn remove_generation_dir(&mut self, path: &Path) -> Result<()> {
        if self.remove_owned_path(path)? {
            Ok(())
        } else {
            bail!(
                "generation directory disappeared before removal: {}",
                path.display()
            )
        }
    }

    fn remove_vector_generation(&mut self, generation: &str) -> Result<bool> {
        self.remove_owned_path(&self.collections_dir.join(generation))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRemovalResult {
    pub generation: String,
    pub semantic_generation: String,
    pub removed_paths: Vec<PathBuf>,
    pub semantic_generation_removed: bool,
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
            semantic_generation: bundle.semantic_generation.clone(),
            removed_paths: Vec::new(),
            semantic_generation_removed: false,
            removed_bytes: 0,
            remaining_reclaimable_bytes: bundle.bytes,
            errors: Vec::new(),
        };
        match remover.remove_vector_generation(&bundle.semantic_generation) {
            Ok(true) => {
                result.semantic_generation_removed = true;
                result.removed_bytes = result
                    .removed_bytes
                    .saturating_add(bundle.semantic.as_ref().map_or(0, |item| item.bytes));
            }
            Ok(false) => result.errors.push(format!(
                "delete vector generation {} was acknowledged but its local data remains",
                bundle.semantic_generation
            )),
            Err(error) => result.errors.push(format!(
                "delete vector generation {}: {error:#}",
                bundle.semantic_generation
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
    Semantic,
}

#[derive(Default)]
struct BundleBuilder {
    lexical: Option<GenerationArtifact>,
    scip: Option<GenerationArtifact>,
    semantic: Option<GenerationArtifact>,
}

impl BundleBuilder {
    fn set(&mut self, kind: ArtifactKind, artifact: GenerationArtifact) {
        match kind {
            ArtifactKind::Lexical => self.lexical = Some(artifact),
            ArtifactKind::Scip => self.scip = Some(artifact),
            ArtifactKind::Semantic => self.semantic = Some(artifact),
        }
    }

    fn finish(
        self,
        project_id: &str,
        generation: String,
        state: GenerationRetentionState,
    ) -> GenerationBundle {
        let bytes = [&self.lexical, &self.scip, &self.semantic]
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
            semantic_generation: format!("codestory_{project_id}_{suffix}"),
            state,
            lexical: self.lexical,
            scip: self.scip,
            semantic: self.semantic,
            bytes,
        }
    }
}

/// What one entry inside the retention directory means to this reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetentionDirEntry {
    /// A marker file to parse.
    Marker,
    /// A lock file or an in-flight atomic temporary: never protection
    /// evidence, so skipping it loses nothing.
    Ignorable,
    /// Something inside a CodeStory-owned evidence directory that this reader
    /// does not understand — treated as missing protection, not as absence.
    Unrecognized,
}

pub(crate) fn classify_retention_entry(path: &Path) -> RetentionDirEntry {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return RetentionDirEntry::Unrecognized;
    };
    // `write_file_atomic` publishes through a dot-prefixed sibling, and a
    // crashed writer can leave one behind. Hidden files are likewise not
    // retention evidence.
    if name.starts_with('.') {
        return RetentionDirEntry::Ignorable;
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some(RETENTION_MARKER_EXTENSION) => RetentionDirEntry::Marker,
        Some(RETENTION_LOCK_EXTENSION) => RetentionDirEntry::Ignorable,
        _ => RetentionDirEntry::Unrecognized,
    }
}

pub(crate) fn retention_dir(state_file: &Path) -> PathBuf {
    state_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(RETENTION_DIR)
}

pub(crate) fn ensure_retention_dir(state_file: &Path) -> Result<PathBuf> {
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
    // Dual-read: a peer on the previous binary still writes schema 1 and must
    // keep pinning what it serves, so both schemas parse. Anything outside the
    // known range is evidence this reader cannot interpret and is refused, so
    // the caller protects instead of ignoring it.
    match marker.schema_version {
        RETENTION_MARKER_SCHEMA_V1 => {
            if marker.owner.is_some()
                || marker.generation.is_some()
                || marker.heartbeat_epoch_ms.is_some()
            {
                bail!("schema 1 generation retention marker carries schema 2 registration fields");
            }
        }
        RETENTION_MARKER_SCHEMA_V2 => {
            let owner = marker
                .owner
                .as_ref()
                .context("schema 2 generation retention marker is missing its workspace owner")?;
            if owner.workspace_id != marker.workspace_id {
                bail!(
                    "generation retention marker owner workspace does not match marker workspace"
                );
            }

            if owner.project_root.is_empty() {
                bail!("generation retention marker owner has an empty project root");
            }
            marker
                .heartbeat_epoch_ms
                .context("schema 2 generation retention marker is missing its heartbeat")?;
        }
        _ => bail!("unsupported generation retention marker schema"),
    }
    validate_retention_component(&marker.workspace_id)?;
    if marker.project_id != marker.active.project_id {
        bail!("generation retention marker active project does not match marker project");
    }
    let active_generation = canonical_manifest_generation(&marker.active)?;
    if marker
        .generation
        .as_ref()
        .is_some_and(|generation| generation != &active_generation)
    {
        bail!("generation retention marker generation does not match its active manifest");
    }
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

pub(crate) fn read_marker_path(path: &Path) -> Result<Option<GenerationRetentionMarker>> {
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
    if manifest.semantic_generation != format!("codestory_{}_{suffix}", manifest.project_id) {
        bail!("retrieval manifest generation and semantic vector generation do not match");
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

/// Every store that may hold protection evidence. A candidate this scan cannot
/// even enumerate is missing evidence, so it marks the scan incomplete rather
/// than shrinking the protected set silently.
fn storage_paths_for_scan(
    cache_root: &Path,
    active_storage_path: Option<&Path>,
    scan: &mut RetentionProtectionScan,
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    if let Some(path) = active_storage_path {
        insert_direct_storage_path(path, "active storage", &mut paths, scan);
    }
    let flat = cache_root.join("codestory.db");
    insert_direct_storage_path(&flat, "flat cache storage", &mut paths, scan);
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
                                scan.record_incomplete(format!(
                                    "read cache entry type {}: {error}",
                                    entry.path().display()
                                ));
                                continue;
                            }
                        };
                        if file_type.is_symlink() {
                            scan.record_incomplete(format!(
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
                            scan,
                        );
                    }
                    Err(error) => scan.record_incomplete(format!(
                        "read cache entry under {}: {error}",
                        cache_root.display()
                    )),
                }
            }
        }
        Err(error) => scan.record_incomplete(format!(
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
    scan: &mut RetentionProtectionScan,
) {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => scan
            .record_incomplete(format!(
                "{label} is not a direct regular file: {}",
                path.display()
            )),
        Ok(_) => {
            paths.insert(path.to_path_buf());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            scan.record_incomplete(format!("inspect {label} {}: {error}", path.display()))
        }
    }
}

fn deduplicate_manifests(manifests: &mut Vec<RetrievalIndexManifest>) {
    manifests.sort_by(|left, right| {
        (
            &left.project_id,
            &left.sidecar_generation,
            &left.semantic_generation,
        )
            .cmp(&(
                &right.project_id,
                &right.sidecar_generation,
                &right.semantic_generation,
            ))
    });
    manifests.dedup_by(|left, right| {
        left.project_id == right.project_id
            && left.sidecar_generation == right.sidecar_generation
            && left.semantic_generation == right.semantic_generation
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

fn discover_vector_generations(
    root: &Path,
    project_id: &str,
    builders: &mut BTreeMap<String, BundleBuilder>,
    blocked: &mut Vec<BlockedGenerationEntry>,
    errors: &mut Vec<String>,
) {
    let Some(entries) = read_direct_directory(root, "semantic vector generation root", errors)
    else {
        return;
    };
    let project_prefix = format!("codestory_{project_id}_");
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!(
                    "read semantic vector generation in {}: {error}",
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
            block_scoped_entry(
                path,
                "malformed semantic vector generation name",
                blocked,
                errors,
            );
            continue;
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                errors.push(format!(
                    "read semantic vector generation type {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            block_scoped_entry(
                path,
                "semantic vector generation entry is not a direct directory",
                blocked,
                errors,
            );
            continue;
        }
        match directory_size(&path) {
            Ok(bytes) => builders
                .entry(format!("{project_id}-{suffix}"))
                .or_default()
                .set(ArtifactKind::Semantic, GenerationArtifact { path, bytes }),
            Err(error) => errors.push(format!(
                "measure semantic vector generation {}: {error:#}",
                path.display()
            )),
        }
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

pub(crate) fn directory_size(path: &Path) -> Result<u64> {
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
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn manifest(project_id: &str, suffix: &str, built_at_epoch_ms: i64) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: project_id.into(),
            lexical_version: "v1".into(),
            semantic_generation: format!("codestory_{project_id}_{suffix}"),
            scip_revision: Some(format!("graph-{suffix}")),
            built_at_epoch_ms,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
            embedding_dim: Some(768),
            sidecar_schema_version: Some(2),
            sidecar_input_hash: Some(suffix.repeat(4)),
            sidecar_generation: Some(format!("{project_id}-{suffix}")),
            projection_count: Some(1),
            symbol_doc_count: Some(1),
            dense_projection_count: Some(1),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
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
            lexical_data_dir: root.join("lexical"),
            semantic_data_dir: root.join("semantic"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("retrieval-sidecars.json"),
        }
    }

    fn write_bundle(layout: &SidecarLayout, project_id: &str, suffix: &str, sizes: [usize; 3]) {
        let generation = format!("{project_id}-{suffix}");
        let lexical = layout.lexical_data_dir.join("shards").join(&generation);
        let scip = layout.scip_artifacts_root.join(&generation);
        let semantic = layout
            .semantic_data_dir
            .join("collections")
            .join(format!("codestory_{project_id}_{suffix}"));
        for (dir, size) in [
            (&lexical, sizes[0]),
            (&scip, sizes[1]),
            (&semantic, sizes[2]),
        ] {
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
            ..SidecarRuntimeConfig::local()
        };
        let local = runtime(
            SidecarProfile::Local,
            root.path().join("retrieval-sidecars.json"),
        );
        let agent = runtime(
            SidecarProfile::Agent,
            root.path()
                .join("retrieval")
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
        semantic_data_remains: bool,
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

        fn remove_vector_generation(&mut self, collection: &str) -> Result<bool> {
            self.removed_collections.push(collection.to_string());
            Ok(!self.semantic_data_remains)
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
                root.path(),
                manifest(project, active, 10),
                Some(RetrievalIndexRollbackRecord {
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
    fn acknowledged_semantic_delete_does_not_overstate_removed_bytes_when_data_remains() {
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
            semantic_data_remains: true,
            ..TestRemover::default()
        };

        let report = apply_generation_retention(&plan, &mut remover);

        assert_eq!(report.removed_bytes, 5);
        assert_eq!(report.remaining_reclaimable_bytes, 4);
        assert!(!report.removals[0].semantic_generation_removed);
        assert!(report.errors[0].contains("local data remains"));
    }

    #[test]
    fn marker_update_preserves_only_a_freshly_verified_rollback() {
        let root = tempdir().expect("root");
        let project = "repo-v1-project";
        let active = manifest(project, "aaaaaaaaaaaaaaaa", 10);
        let rollback = RetrievalIndexRollbackRecord {
            manifest: manifest(project, "bbbbbbbbbbbbbbbb", 9),
            verified_at_epoch_ms: 11,
        };
        let mut refreshed_active = active;
        refreshed_active.built_at_epoch_ms = 20;

        let refreshed = GenerationRetentionMarker::next(
            "workspace_1",
            root.path(),
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
            root.path(),
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
        assert!(scan.protection_incomplete);
    }

    /// A second OS process holds the retention lock for longer than the
    /// caller's budget. Before bounded acquisition the caller blocked in
    /// `flock` for the holder's whole lifetime, so an eviction or shutdown
    /// waiting behind a routine sibling publication never returned.
    #[test]
    fn a_second_process_holding_the_retention_lock_cannot_outlive_the_caller_budget() {
        const HOLD_ENV: &str = "CODESTORY_TEST_HOLD_RETENTION_LOCK";
        const HOLD_MS_ENV: &str = "CODESTORY_TEST_HOLD_RETENTION_LOCK_MS";
        const READY_ENV: &str = "CODESTORY_TEST_HOLD_RETENTION_LOCK_READY";

        if let Some(state_file) = std::env::var_os(HOLD_ENV) {
            let state_file = PathBuf::from(state_file);
            let lock = GenerationRetentionLock::acquire(&state_file, "held_scope")
                .expect("child acquires the retention lock");
            std::fs::write(
                PathBuf::from(std::env::var_os(READY_ENV).expect("ready marker path")),
                b"held",
            )
            .expect("publish holder readiness");
            let hold_ms: u64 = std::env::var(HOLD_MS_ENV)
                .expect("hold budget")
                .parse()
                .expect("numeric hold budget");
            std::thread::sleep(Duration::from_millis(hold_ms));
            drop(lock);
            return;
        }

        let root = tempdir().expect("root");
        let state_file = root.path().join("retrieval-sidecars.json");
        let ready = root.path().join("holder.ready");
        let mut holder = std::process::Command::new(
            std::env::current_exe().expect("current test executable"),
        )
        .arg("--exact")
        .arg("retention::tests::a_second_process_holding_the_retention_lock_cannot_outlive_the_caller_budget")
        .arg("--nocapture")
        .env(HOLD_ENV, &state_file)
        .env(READY_ENV, &ready)
        .env(HOLD_MS_ENV, "5000")
        .spawn()
        .expect("spawn holder process");

        let holder_deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() && Instant::now() < holder_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists(), "the holder process never took the lock");

        let started = Instant::now();
        let error = GenerationRetentionLock::acquire_bounded(
            &state_file,
            "held_scope",
            FileLockKind::Exclusive,
            LockDeadline::after(Duration::from_millis(250)),
            None,
        )
        .expect_err("a cross-process holder must not block the caller");
        let waited = started.elapsed();

        assert!(
            error
                .to_string()
                .contains("acquire exclusive generation retention"),
            "the refusal must name the contended lock: {error:#}"
        );
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("lock_wait_timeout")),
            "the refusal must carry the typed timeout code: {error:#}"
        );
        assert!(
            waited < Duration::from_secs(3),
            "acquisition waited {waited:?}, far past its 250 ms budget"
        );

        holder.wait().expect("holder process exits");
    }

    /// A publication pass routinely holds this lock longer than
    /// [`codestory_contracts::bounded_locks::DEFAULT_LOCK_WAIT`]. A waiter that
    /// refuses at ten seconds turns ordinary contention behind a legitimate
    /// commit into a hard failure, so the shared side carries the publication
    /// budget — and stays interruptible, which is the only reason a budget that
    /// long is safe.
    ///
    /// Both halves are proven at once: past the ten-second mark the wait is
    /// still running (so the budget is not the default one) and it then ends on
    /// the cancellation flag rather than on a timeout.
    #[test]
    fn a_publication_length_hold_is_waited_out_rather_than_refused_at_the_default_budget() {
        use codestory_contracts::bounded_locks::DEFAULT_LOCK_WAIT;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let root = tempdir().expect("root");
        let state_file = root.path().join("retrieval-sidecars.json");
        let _holder = GenerationRetentionLock::acquire(&state_file, "long_publication")
            .expect("a peer takes the lock for its publication pass");

        let cancel = Arc::new(AtomicBool::new(false));
        let raiser = Arc::clone(&cancel);
        let past_the_default_budget = DEFAULT_LOCK_WAIT + Duration::from_millis(750);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(past_the_default_budget);
            raiser.store(true, Ordering::Release);
        });

        let started = Instant::now();
        let waiter_state_file = state_file.clone();
        let waiter_cancel = Arc::clone(&cancel);
        let waiter = std::thread::spawn(move || {
            // The ambient scope stands in for the activation worker's: the
            // production call site below passes no flag of its own.
            codestory_contracts::bounded_locks::with_thread_cancellation(waiter_cancel, || {
                GenerationRetentionLock::acquire_shared(&waiter_state_file, "long_publication")
            })
        });
        let outcome = waiter.join().expect("waiter thread");
        let waited = started.elapsed();
        canceller.join().expect("cancellation thread");

        let error = outcome.expect_err("the peer never released, so the wait cannot succeed");
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("lock_wait_cancelled")),
            "the wait must end on cancellation, not on a budget shorter than the peer's pass: {error:#}"
        );
        assert!(
            waited >= DEFAULT_LOCK_WAIT,
            "the wait gave up after {waited:?}, inside the {DEFAULT_LOCK_WAIT:?} foreground budget, so a legitimate publication would be refused"
        );
    }

    /// A peer already running the schema-2 writer publishes a marker into the
    /// shared retention directory. Before dual-read this reader rejected the
    /// unknown schema outright, so the peer's live generation became an
    /// unrooted reclaim candidate and the whole project stopped pruning.
    #[test]
    fn a_schema_2_marker_from_a_peer_roots_its_generation_for_a_dual_reading_scanner() {
        let root = tempdir().expect("root");
        let peer_workspace_root = root.path().join("peer-worktree");
        std::fs::create_dir_all(&peer_workspace_root).expect("peer worktree");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let peer = "aaaaaaaaaaaaaaaa";
        let stale = "cccccccccccccccc";
        write_bundle(&layout, project, peer, [1, 1, 1]);
        write_bundle(&layout, project, stale, [1, 1, 1]);
        let state_file = layout.state_file.clone();
        let marker = GenerationRetentionMarker::next(
            "workspace_peer",
            &peer_workspace_root,
            manifest(project, peer, 10),
            None,
            10,
        )
        .expect("schema 2 marker");
        assert_eq!(marker.schema_version, RETENTION_MARKER_SCHEMA_V2);
        write_retention_marker(&state_file, &marker).expect("write marker");

        let protection = scan_retention_protection(root.path(), None, &state_file);
        let plan = plan_generation_retention(&layout, project, &protection);

        assert!(
            !protection.protection_incomplete,
            "a schema 2 marker is readable evidence, not a gap: {:?}",
            protection.errors
        );
        assert_eq!(
            plan.bundles
                .iter()
                .map(|bundle| (bundle.generation.as_str(), bundle.state))
                .collect::<Vec<_>>(),
            vec![
                (
                    "repo-v1-project-aaaaaaaaaaaaaaaa",
                    GenerationRetentionState::Active
                ),
                (
                    "repo-v1-project-cccccccccccccccc",
                    GenerationRetentionState::Reclaimable
                ),
            ]
        );
        assert!(!plan.pruning_suppressed);
    }

    /// Dual-read runs the other way too: a peer still on the previous binary
    /// writes schema 1, and its generation must keep its pin.
    #[test]
    fn a_schema_1_marker_still_roots_its_generation() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let legacy = "bbbbbbbbbbbbbbbb";
        write_bundle(&layout, project, legacy, [1, 1, 1]);
        let state_file = layout.state_file.clone();
        let legacy_marker = GenerationRetentionMarker {
            schema_version: RETENTION_MARKER_SCHEMA_V1,
            workspace_id: "workspace_legacy".into(),
            project_id: project.into(),
            active: manifest(project, legacy, 10),
            rollback: None,
            updated_at_epoch_ms: 10,
            owner: None,
            generation: None,
            heartbeat_epoch_ms: None,
        };
        ensure_retention_dir(&state_file).expect("retention dir");
        std::fs::write(
            retention_marker_path(&state_file, "workspace_legacy").expect("marker path"),
            serde_json::to_vec(&legacy_marker).expect("serialize legacy marker"),
        )
        .expect("write legacy marker");

        let protection = scan_retention_protection(root.path(), None, &state_file);
        let plan = plan_generation_retention(&layout, project, &protection);

        assert!(!protection.protection_incomplete, "{:?}", protection.errors);
        assert_eq!(
            plan.bundles
                .iter()
                .map(|bundle| (bundle.generation.as_str(), bundle.state))
                .collect::<Vec<_>>(),
            vec![(
                "repo-v1-project-bbbbbbbbbbbbbbbb",
                GenerationRetentionState::Active
            )]
        );
        assert_eq!(
            legacy_marker.retirement(),
            MarkerRetirement::UnregisteredWorkspace
        );
    }

    /// The point of the registration: a worktree that has been deleted stops
    /// pinning, while a worktree that still exists keeps its pin no matter how
    /// old its marker is.
    #[test]
    fn a_deleted_worktree_releases_its_generation_while_a_live_one_keeps_it() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let live_root = root.path().join("live-worktree");
        let dead_root = root.path().join("dead-worktree");
        std::fs::create_dir_all(&live_root).expect("live worktree");
        std::fs::create_dir_all(&dead_root).expect("dead worktree");
        let live = "aaaaaaaaaaaaaaaa";
        let dead = "dddddddddddddddd";
        write_bundle(&layout, project, live, [1, 1, 1]);
        write_bundle(&layout, project, dead, [1, 1, 1]);
        let state_file = layout.state_file.clone();
        for (workspace, worktree, suffix, age) in [
            ("workspace_live", &live_root, live, 1),
            ("workspace_dead", &dead_root, dead, 9_999),
        ] {
            let marker = GenerationRetentionMarker::next(
                workspace,
                worktree,
                manifest(project, suffix, age),
                None,
                age,
            )
            .expect("marker");
            write_retention_marker(&state_file, &marker).expect("write marker");
        }
        std::fs::remove_dir_all(&dead_root).expect("retire the dead worktree");

        let protection = scan_retention_protection(root.path(), None, &state_file);
        let plan = plan_generation_retention(&layout, project, &protection);

        assert!(!protection.protection_incomplete, "{:?}", protection.errors);
        assert_eq!(
            protection
                .retired_marker_paths
                .iter()
                .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
                .collect::<Vec<_>>(),
            vec!["workspace_dead.json"]
        );
        assert_eq!(
            plan.bundles
                .iter()
                .map(|bundle| (bundle.generation.as_str(), bundle.state))
                .collect::<Vec<_>>(),
            vec![
                (
                    "repo-v1-project-aaaaaaaaaaaaaaaa",
                    GenerationRetentionState::Active
                ),
                (
                    "repo-v1-project-dddddddddddddddd",
                    GenerationRetentionState::Reclaimable
                ),
            ]
        );
    }

    /// A future marker encoding this reader does not recognize sits in the
    /// same owned directory. Skipping it reads as "no roots", which is the one
    /// interpretation that deletes a live pin, so it must protect instead.
    #[test]
    fn an_unrecognized_retention_entry_protects_instead_of_being_skipped() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let live_root = root.path().join("live-worktree");
        std::fs::create_dir_all(&live_root).expect("live worktree");
        let pinned = "aaaaaaaaaaaaaaaa";
        let other = "cccccccccccccccc";
        write_bundle(&layout, project, pinned, [1, 1, 1]);
        write_bundle(&layout, project, other, [1, 1, 1]);
        let state_file = layout.state_file.clone();
        let marker = GenerationRetentionMarker::next(
            "workspace_live",
            &live_root,
            manifest(project, pinned, 10),
            None,
            10,
        )
        .expect("marker");
        write_retention_marker(&state_file, &marker).expect("write marker");
        // A marker written in an encoding a later release introduces.
        std::fs::write(
            retention_dir(&state_file).join("workspace_future.marker3"),
            b"opaque",
        )
        .expect("future marker");

        let protection = scan_retention_protection(root.path(), None, &state_file);
        let plan = plan_generation_retention(&layout, project, &protection);

        assert!(
            protection.protection_incomplete,
            "an entry this reader cannot interpret is missing protection, not absent protection"
        );
        assert!(
            protection
                .errors
                .iter()
                .any(|error| error.contains("workspace_future.marker3")),
            "{:?}",
            protection.errors
        );
        assert!(plan.pruning_suppressed);
        assert_eq!(
            plan.bundles
                .iter()
                .find(|bundle| bundle.generation.ends_with(other))
                .map(|bundle| bundle.state),
            Some(GenerationRetentionState::Building),
            "no generation may be reclaimable while protection evidence is unreadable"
        );
    }

    /// Lock files and crashed atomic-write temporaries share the directory and
    /// are not protection evidence, so they must not wedge pruning forever.
    #[test]
    fn locks_and_abandoned_temporaries_are_not_unreadable_protection() {
        let root = tempdir().expect("root");
        let layout = layout(root.path());
        let project = "repo-v1-project";
        let live_root = root.path().join("live-worktree");
        std::fs::create_dir_all(&live_root).expect("live worktree");
        write_bundle(&layout, project, "aaaaaaaaaaaaaaaa", [1, 1, 1]);
        let state_file = layout.state_file.clone();
        let marker = GenerationRetentionMarker::next(
            "workspace_live",
            &live_root,
            manifest(project, "aaaaaaaaaaaaaaaa", 10),
            None,
            10,
        )
        .expect("marker");
        write_retention_marker(&state_file, &marker).expect("write marker");
        let dir = retention_dir(&state_file);
        std::fs::write(dir.join("repo-v1-project.lock"), b"").expect("lock file");
        std::fs::write(
            dir.join(".generation-retention.4242.7.tmp"),
            b"abandoned partial write",
        )
        .expect("abandoned temporary");

        let protection = scan_retention_protection(root.path(), None, &state_file);

        assert!(
            !protection.protection_incomplete,
            "locks and temporaries are not evidence: {:?}",
            protection.errors
        );
        assert!(protection.errors.is_empty(), "{:?}", protection.errors);
    }

    /// A schema-2 marker whose registration was copied from another workspace
    /// would retire the wrong cache, so it is refused rather than trusted.
    #[test]
    fn a_marker_registration_for_another_workspace_is_refused() {
        let root = tempdir().expect("root");
        let project = "repo-v1-project";
        let mut marker = GenerationRetentionMarker::next(
            "workspace_1",
            root.path(),
            manifest(project, "aaaaaaaaaaaaaaaa", 1),
            None,
            2,
        )
        .expect("marker");
        marker.owner = Some(RetentionMarkerOwner {
            workspace_id: "workspace_2".into(),
            project_root: root.path().display().to_string(),
        });

        let error = validate_marker(&marker).expect_err("mismatched registration is refused");

        assert!(
            error
                .to_string()
                .contains("owner workspace does not match marker workspace"),
            "{error:#}"
        );
    }
}
