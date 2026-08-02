//! Proof-owned cleanup for cache-root state no live workspace can still claim.
//!
//! Generation retention already reclaims superseded generations inside one
//! project's sidecar tree. Two things sit outside it and were never reclaimed:
//! the per-project cache directory of a worktree that no longer exists, and the
//! content-addressed model tree of a model revision this executable was not
//! built with.
//!
//! Both are removed only against positive proof, never against a heuristic.
//! A project cache is removable only when a schema-2 retention marker
//! registered a project root that is now provably gone *and* the directory was
//! read end to end without finding one authored annotation; a model tree is
//! removable only when its digest differs from the digest compiled into this
//! binary and no peer holds its materialization lock. Everything else is
//! reported as retained with a typed reason, so the plan reads as an audit
//! rather than as a list of guesses.
//!
//! Two proof limits are deliberate and named rather than implied:
//!
//! - A model tree proves "not the revision this executable was built with" and
//!   "not being materialized right now". It does not prove that no separately
//!   installed CodeStory executable is running with that revision mapped.
//!   Reclamation is still safe on POSIX, where unlinking an open file leaves
//!   the mapping intact, and fails loudly on Windows, where the open handle
//!   refuses the delete; a peer never observes a truncated model.
//! - Absence of authored annotations must be *read*, never inferred from a
//!   missing file. Anything this reader cannot open, parse, or enumerate
//!   leaves the cache in place.

use crate::config::{SidecarRuntimeConfig, user_cache_root};
use crate::retention::{
    GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionLock, MarkerRetirement, RetentionDirEntry,
    classify_retention_entry, directory_size, global_generation_gc_state_file, read_marker_path,
    retention_dir,
};
use anyhow::{Context, Result, anyhow};
use codestory_contracts::bounded_locks::{self, FileLockKind};
use codestory_contracts::owned_artifacts::{
    ANNOTATIONS_SIDECAR_FILE, EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE, ROLLBACK_BACKUP_EXTENSION,
    SQLITE_SIDECAR_SUFFIXES, embedded_model_digest_root,
};
use codestory_store::Store;
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Wire version of the plan and report documents.
pub const CACHE_CLEAN_SCHEMA_VERSION: u32 = 1;

/// A workspace cache directory is named by the 16-hex workspace identity.
/// Anything else under the cache root is a different artifact family and is
/// never a project-cache candidate.
const WORKSPACE_ID_HEX_LEN: usize = 16;
/// Content-addressed model directories are named by a SHA-256 digest.
const MODEL_DIGEST_HEX_LEN: usize = 64;

/// What kind of proof made one entry removable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCleanKind {
    /// A per-project cache directory whose registered workspace is gone.
    AbandonedProjectCache,
    /// A materialized model tree for a digest this executable does not use.
    SupersededModelDigest,
}

/// Why one entry was left in place. Every refusal is typed so an operator can
/// tell "nothing to do" apart from "refused for cause".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCleanRefusal {
    /// The registered project root still exists.
    LiveWorkspace,
    /// No schema-2 registration exists for this cache, so abandonment cannot
    /// be proven. Covers both a missing marker and a schema-1 marker.
    UnregisteredWorkspace,
    /// The registered project root could not be observed either way.
    UnprovenWorkspace,
    /// The digest this executable was built with.
    CurrentModelDigest,
    /// Inside a CodeStory-owned tree but not a shape this planner recognizes.
    UnrecognizedEntry,
    /// A proven-abandoned cache holds authored annotations, in the versioned
    /// sidecar or in the core store's own `bookmark_category` /
    /// `bookmark_node` tables. Their contents are authored, not derived, so a
    /// lost workspace cannot regenerate them and cleanup will not take them.
    AnnotationsPresent,
    /// A proven-abandoned cache could not be *read* well enough to prove it
    /// holds no authored annotation. Absence is never inferred from a missing
    /// file, so an unreadable or unopenable cache is refused.
    AnnotationsUnproven,
    /// A model tree whose materialization lock a peer currently holds.
    ModelTreeInUse,
    /// The entry could not be measured or inspected safely.
    UnsafeEntry,
}

impl CacheCleanRefusal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LiveWorkspace => "live_workspace",
            Self::UnregisteredWorkspace => "unregistered_workspace",
            Self::UnprovenWorkspace => "unproven_workspace",
            Self::CurrentModelDigest => "current_model_digest",
            Self::UnrecognizedEntry => "unrecognized_entry",
            Self::AnnotationsPresent => "annotations_present",
            Self::AnnotationsUnproven => "annotations_unproven",
            Self::ModelTreeInUse => "model_tree_in_use",
            Self::UnsafeEntry => "unsafe_entry",
        }
    }
}

/// One removable entry and the proof that made it removable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCleanCandidate {
    pub kind: CacheCleanKind,
    /// Path relative to the cache root; also the owned deletion key.
    pub relative_path: String,
    pub bytes: u64,
    pub proof: String,
}

/// One entry deliberately left in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCleanRetained {
    pub relative_path: String,
    pub reason: CacheCleanRefusal,
    pub detail: String,
}

/// The auditable cleanup plan. Building it never writes to the cache tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCleanPlan {
    pub schema_version: u32,
    pub dry_run: bool,
    pub cache_root: String,
    pub current_model_digest: String,
    pub reclaimable_bytes: u64,
    pub candidates: Vec<CacheCleanCandidate>,
    pub retained: Vec<CacheCleanRetained>,
    pub errors: Vec<String>,
}

/// One attempted removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCleanRemoval {
    pub kind: CacheCleanKind,
    pub relative_path: String,
    pub bytes: u64,
    pub removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The result of applying a plan re-derived under the cleanup lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCleanReport {
    pub schema_version: u32,
    pub dry_run: bool,
    pub plan: CacheCleanPlan,
    pub removed_bytes: u64,
    pub removals: Vec<CacheCleanRemoval>,
    pub errors: Vec<String>,
}

/// Plan cleanup for the process cache root without touching it.
pub fn plan_cache_clean() -> Result<CacheCleanPlan> {
    let cache_root = user_cache_root();
    Ok(build_cache_clean_plan(&cache_root))
}

/// Apply cleanup for the process cache root.
///
/// The plan is re-derived while holding the global generation cleanup lock, so
/// a publication that started after an operator read the dry run cannot have
/// its evidence deleted underneath it.
pub fn apply_cache_clean() -> Result<CacheCleanReport> {
    let cache_root = user_cache_root();
    let runtime = SidecarRuntimeConfig::local();
    let _global_gc_lock = GenerationRetentionLock::acquire(
        &global_generation_gc_state_file(&runtime),
        GLOBAL_GENERATION_GC_LOCK_SCOPE,
    )
    .context("coordinate cache cleanup with generation publication")?;
    let mut plan = build_cache_clean_plan(&cache_root);
    plan.dry_run = false;
    Ok(apply_cache_clean_plan(&cache_root, plan))
}

fn apply_cache_clean_plan(cache_root: &Path, plan: CacheCleanPlan) -> CacheCleanReport {
    let mut removals = Vec::new();
    let mut removed_bytes = 0_u64;
    let mut errors = plan.errors.clone();
    match OwnedDeletionRoot::open(cache_root) {
        Ok(deletion) => {
            for candidate in &plan.candidates {
                let relative = PathBuf::from(&candidate.relative_path);
                match deletion.remove(&relative) {
                    Ok(true) => {
                        removed_bytes = removed_bytes.saturating_add(candidate.bytes);
                        removals.push(CacheCleanRemoval {
                            kind: candidate.kind,
                            relative_path: candidate.relative_path.clone(),
                            bytes: candidate.bytes,
                            removed: true,
                            error: None,
                        });
                    }
                    Ok(false) => removals.push(CacheCleanRemoval {
                        kind: candidate.kind,
                        relative_path: candidate.relative_path.clone(),
                        bytes: 0,
                        removed: false,
                        error: Some("entry disappeared before removal".into()),
                    }),
                    Err(error) => {
                        let message = format!("remove {}: {error}", candidate.relative_path);
                        errors.push(message.clone());
                        removals.push(CacheCleanRemoval {
                            kind: candidate.kind,
                            relative_path: candidate.relative_path.clone(),
                            bytes: 0,
                            removed: false,
                            error: Some(message),
                        });
                    }
                }
            }
        }
        Err(error) => errors.push(format!(
            "open owned cache root {}: {error}",
            cache_root.display()
        )),
    }
    CacheCleanReport {
        schema_version: CACHE_CLEAN_SCHEMA_VERSION,
        dry_run: false,
        plan,
        removed_bytes,
        removals,
        errors,
    }
}

/// Registration evidence for one workspace cache, read from the retention
/// marker directory.
struct WorkspaceRegistration {
    retirement: MarkerRetirement,
    project_root: Option<String>,
}

fn build_cache_clean_plan(cache_root: &Path) -> CacheCleanPlan {
    let mut errors = Vec::new();
    let mut candidates = Vec::new();
    let mut retained = Vec::new();
    let registrations = read_workspace_registrations(cache_root, &mut errors);
    plan_project_caches(
        cache_root,
        &registrations,
        &mut candidates,
        &mut retained,
        &mut errors,
    );
    plan_model_digests(cache_root, &mut candidates, &mut retained, &mut errors);
    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    retained.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let reclaimable_bytes = candidates.iter().fold(0_u64, |total, candidate| {
        total.saturating_add(candidate.bytes)
    });
    CacheCleanPlan {
        schema_version: CACHE_CLEAN_SCHEMA_VERSION,
        dry_run: true,
        cache_root: cache_root.display().to_string(),
        current_model_digest: codestory_llama_sys::MODEL_SHA256.to_string(),
        reclaimable_bytes,
        candidates,
        retained,
        errors,
    }
}

/// Read every retention marker under the local-profile state file and index it
/// by the workspace it registers.
fn read_workspace_registrations(
    cache_root: &Path,
    errors: &mut Vec<String>,
) -> BTreeMap<String, WorkspaceRegistration> {
    let mut registrations = BTreeMap::new();
    let marker_dir = retention_dir(&local_state_file(cache_root));
    let entries = match std::fs::read_dir(&marker_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return registrations,
        Err(error) => {
            errors.push(format!(
                "read retention marker directory {}: {error}",
                marker_dir.display()
            ));
            return registrations;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!(
                    "read retention marker entry in {}: {error}",
                    marker_dir.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        if classify_retention_entry(&path) != RetentionDirEntry::Marker {
            continue;
        }
        match read_marker_path(&path) {
            Ok(Some(marker)) => {
                let retirement = marker.retirement();
                registrations.insert(
                    marker.workspace_id.clone(),
                    WorkspaceRegistration {
                        retirement,
                        project_root: marker.owner.map(|owner| owner.project_root),
                    },
                );
            }
            Ok(None) => {}
            Err(error) => errors.push(format!(
                "read retention marker {}: {error:#}",
                path.display()
            )),
        }
    }
    registrations
}

/// The local profile roots its retention state directly in the cache root, so
/// every workspace registration for this machine sits under one marker
/// directory regardless of which project wrote it.
fn local_state_file(cache_root: &Path) -> PathBuf {
    cache_root.join(crate::config::RETRIEVAL_STATE_FILE)
}

fn plan_project_caches(
    cache_root: &Path,
    registrations: &BTreeMap<String, WorkspaceRegistration>,
    candidates: &mut Vec<CacheCleanCandidate>,
    retained: &mut Vec<CacheCleanRetained>,
    errors: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(cache_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            errors.push(format!("read cache root {}: {error}", cache_root.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!(
                    "read cache entry under {}: {error}",
                    cache_root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_workspace_cache_name(&name) {
            continue;
        }
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() && !file_type.is_symlink() => {}
            Ok(_) => {
                retained.push(CacheCleanRetained {
                    relative_path: name,
                    reason: CacheCleanRefusal::UnsafeEntry,
                    detail: "workspace cache entry is not a direct directory".into(),
                });
                continue;
            }
            Err(error) => {
                errors.push(format!("read cache entry type {}: {error}", path.display()));
                continue;
            }
        }

        let Some(registration) = registrations.get(&name) else {
            retained.push(CacheCleanRetained {
                relative_path: name,
                reason: CacheCleanRefusal::UnregisteredWorkspace,
                detail: "no schema 2 retention marker registers this workspace cache".into(),
            });
            continue;
        };
        let refusal = match registration.retirement {
            MarkerRetirement::RetiredWorkspace => None,
            MarkerRetirement::LiveWorkspace => Some(CacheCleanRefusal::LiveWorkspace),
            MarkerRetirement::UnprovenWorkspace => Some(CacheCleanRefusal::UnprovenWorkspace),
            MarkerRetirement::UnregisteredWorkspace => {
                Some(CacheCleanRefusal::UnregisteredWorkspace)
            }
        };
        if let Some(reason) = refusal {
            retained.push(CacheCleanRetained {
                relative_path: name,
                reason,
                detail: format!(
                    "registered project root {} is {}",
                    registration.project_root.as_deref().unwrap_or("unrecorded"),
                    registration.retirement.as_str()
                ),
            });
            continue;
        }

        // Authored annotations are the one artifact in a cache directory that
        // a vanished workspace cannot reproduce, so proven abandonment is not
        // enough to take them. Presence is read out of the directory, never
        // inferred from one file name, and anything unreadable refuses.
        match observe_authored_annotations(&path) {
            AnnotationEvidence::ProvenAbsent => {}
            AnnotationEvidence::Present(detail) => {
                retained.push(CacheCleanRetained {
                    relative_path: name,
                    reason: CacheCleanRefusal::AnnotationsPresent,
                    detail,
                });
                continue;
            }
            AnnotationEvidence::Unproven(detail) => {
                retained.push(CacheCleanRetained {
                    relative_path: name,
                    reason: CacheCleanRefusal::AnnotationsUnproven,
                    detail,
                });
                continue;
            }
        }

        match directory_size(&path) {
            Ok(bytes) => candidates.push(CacheCleanCandidate {
                kind: CacheCleanKind::AbandonedProjectCache,
                relative_path: name,
                bytes,
                proof: format!(
                    "registered project root {} no longer exists",
                    registration.project_root.as_deref().unwrap_or("unrecorded")
                ),
            }),
            Err(error) => {
                errors.push(format!("measure cache {}: {error:#}", path.display()));
                retained.push(CacheCleanRetained {
                    relative_path: name,
                    reason: CacheCleanRefusal::UnsafeEntry,
                    detail: format!("cache directory could not be measured: {error:#}"),
                });
            }
        }
    }
}

/// What one cache directory proves about authored annotations.
///
/// Annotations live in two places at once during the sidecar cutover: the
/// versioned annotations sidecar, and the core store's own `bookmark_category`
/// and `bookmark_node` tables for every cache that has not cut over yet. The
/// cutover is lazy -- it happens on an annotation write or a core-replacing
/// operation -- and an abandoned worktree is by definition never re-opened, so
/// its cache is the one population that can *never* have cut over. Testing for
/// the sidecar file therefore proves nothing about exactly the caches this
/// planner is allowed to delete.
#[derive(Debug)]
enum AnnotationEvidence {
    /// Every annotation-bearing artifact under the directory was read and
    /// holds no authored row.
    ProvenAbsent,
    /// Authored annotations exist here.
    Present(String),
    /// Absence could not be proven, so the cache stays.
    Unproven(String),
}

/// Read one cache directory end to end for authored annotations.
///
/// Observation only: stores are opened through the non-mutating observer, so
/// planning cannot migrate, recover, or create a sidecar in a cache this
/// process does not own.
fn observe_authored_annotations(cache_dir: &Path) -> AnnotationEvidence {
    let mut unproven = None;
    if let Some(present) = scan_annotation_evidence(cache_dir, &mut unproven) {
        return AnnotationEvidence::Present(present);
    }
    match unproven {
        Some(detail) => AnnotationEvidence::Unproven(detail),
        None => AnnotationEvidence::ProvenAbsent,
    }
}

/// Walk one directory, returning `Some(detail)` as soon as authored
/// annotations are found.
///
/// Every entry this reader cannot interpret is recorded in `unproven` instead
/// of being skipped, so a directory it could not fully read never reads as
/// empty. The walk is the same shape the byte measurement already performs, so
/// it costs one extra pass over metadata the planner reads anyway.
fn scan_annotation_evidence(dir: &Path, unproven: &mut Option<String>) -> Option<String> {
    let mut paths = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => paths.push(entry.path()),
                    Err(error) => record_unproven(
                        unproven,
                        format!("read entry under {}: {error}", dir.display()),
                    ),
                }
            }
        }
        Err(error) => {
            record_unproven(
                unproven,
                format!("read directory {}: {error}", dir.display()),
            );
            return None;
        }
    }
    paths.sort();
    for path in paths {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                record_unproven(unproven, format!("inspect {}: {error}", path.display()));
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            record_unproven(
                unproven,
                format!("cache holds a linked entry {}", path.display()),
            );
            continue;
        }
        if metadata.is_dir() {
            if let Some(found) = scan_annotation_evidence(&path, unproven) {
                return Some(found);
            }
            continue;
        }
        if !metadata.is_file() {
            record_unproven(
                unproven,
                format!("cache holds an unsupported entry {}", path.display()),
            );
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            record_unproven(
                unproven,
                format!("cache holds an unreadable entry name in {}", dir.display()),
            );
            continue;
        };
        if is_annotations_sidecar_name(name) {
            // This binary does not own the sidecar schema, so its rows cannot
            // be counted here. Refusing on presence is the fail-closed
            // direction: it can only keep a cache, never take one.
            return Some(format!(
                "the authored annotations sidecar {} is present",
                path.display()
            ));
        }
        if let Some(main) = orphaned_sqlite_sidecar_owner(&path, name) {
            record_unproven(
                unproven,
                format!(
                    "{} is a SQLite sidecar whose database {} is missing",
                    path.display(),
                    main.display()
                ),
            );
            continue;
        }
        if !is_core_store_name(name) {
            continue;
        }
        match observe_legacy_annotation_rows(&path) {
            Ok(0) => {}
            Ok(rows) => {
                return Some(format!(
                    "{} holds {rows} authored annotation rows in the core bookmark tables",
                    path.display()
                ));
            }
            Err(error) => record_unproven(
                unproven,
                format!(
                    "core store {} could not be read for authored annotations: {error:#}",
                    path.display()
                ),
            ),
        }
    }
    None
}

/// Keep the first reason absence could not be proven. One is enough to refuse,
/// and the first entry in a sorted walk is stable across platforms.
fn record_unproven(slot: &mut Option<String>, detail: String) {
    if slot.is_none() {
        *slot = Some(detail);
    }
}

/// Count the authored rows in the core store's legacy annotation tables.
fn observe_legacy_annotation_rows(path: &Path) -> Result<u64> {
    let store = Store::open_observational(path)
        .map_err(|error| anyhow!("open observationally: {error}"))?;
    let categories = store
        .get_bookmark_categories()
        .map_err(|error| anyhow!("read bookmark categories: {error}"))?;
    let bookmarks = store
        .get_bookmarks(None)
        .map_err(|error| anyhow!("read bookmarks: {error}"))?;
    Ok(categories.len() as u64 + bookmarks.len() as u64)
}

fn is_annotations_sidecar_name(name: &str) -> bool {
    name == ANNOTATIONS_SIDECAR_FILE
        || SQLITE_SIDECAR_SUFFIXES
            .iter()
            .any(|suffix| name.strip_suffix(suffix) == Some(ANNOTATIONS_SIDECAR_FILE))
}

/// The database a `-wal`, `-shm`, or `-journal` file belongs to, when that
/// database is not present. Orphaned sidecars are evidence this reader cannot
/// interpret rather than derived bytes it may drop.
fn orphaned_sqlite_sidecar_owner(path: &Path, name: &str) -> Option<PathBuf> {
    let base = SQLITE_SIDECAR_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))?;
    let main = path.with_file_name(base);
    (!main.is_file()).then_some(main)
}

/// Whether one file name in a cache directory can carry the core schema, and
/// with it the legacy `bookmark_category` and `bookmark_node` tables. The
/// rollback backup counts: it is a whole core database, and deleting the
/// directory takes it too.
fn is_core_store_name(name: &str) -> bool {
    if name.ends_with(&format!(".{ROLLBACK_BACKUP_EXTENSION}")) {
        return true;
    }
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| CORE_STORE_EXTENSIONS.contains(&extension))
}

/// Extensions a core SQLite database is written with anywhere in the cache
/// root: the live store, staged snapshots, and rehydration copies.
const CORE_STORE_EXTENSIONS: [&str; 3] = ["db", "sqlite", "sqlite3"];

/// Whether a peer currently holds one model tree's materialization lock.
enum ModelTreeLock {
    Free,
    Held,
    Unobservable(String),
}

/// Observe the materialization lock beside one model tree without creating it.
///
/// A dry-run plan must not be the reason the lock file first exists, so the
/// file is opened read-only and a missing file reads as "no holder".
fn observe_model_tree_lock(directory: &Path) -> ModelTreeLock {
    let lock_path = directory.join(EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE);
    let file = match File::open(&lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ModelTreeLock::Free,
        Err(error) => {
            return ModelTreeLock::Unobservable(format!(
                "open materialization lock {}: {error}",
                lock_path.display()
            ));
        }
    };
    match bounded_locks::try_acquire(&file, FileLockKind::Shared) {
        Ok(true) => match bounded_locks::release(&file) {
            Ok(()) => ModelTreeLock::Free,
            Err(error) => ModelTreeLock::Unobservable(format!(
                "release materialization lock {}: {error}",
                lock_path.display()
            )),
        },
        Ok(false) => ModelTreeLock::Held,
        Err(error) => ModelTreeLock::Unobservable(format!(
            "observe materialization lock {}: {error}",
            lock_path.display()
        )),
    }
}

fn plan_model_digests(
    cache_root: &Path,
    candidates: &mut Vec<CacheCleanCandidate>,
    retained: &mut Vec<CacheCleanRetained>,
    errors: &mut Vec<String>,
) {
    let digest_root = embedded_model_digest_root(cache_root);
    let current = codestory_llama_sys::MODEL_SHA256;
    let entries = match std::fs::read_dir(&digest_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            errors.push(format!(
                "read model digest root {}: {error}",
                digest_root.display()
            ));
            return;
        }
    };
    let relative_of = |name: &str| {
        digest_root
            .strip_prefix(cache_root)
            .map(|relative| relative.join(name))
            .unwrap_or_else(|_| PathBuf::from(name))
            .display()
            .to_string()
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!(
                    "read model digest entry in {}: {error}",
                    digest_root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let relative = relative_of(&name);
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() && !file_type.is_symlink() => {}
            Ok(_) => {
                retained.push(CacheCleanRetained {
                    relative_path: relative,
                    reason: CacheCleanRefusal::UnsafeEntry,
                    detail: "model digest entry is not a direct directory".into(),
                });
                continue;
            }
            Err(error) => {
                errors.push(format!(
                    "read model digest type {}: {error}",
                    path.display()
                ));
                continue;
            }
        }
        if !is_model_digest_name(&name) {
            retained.push(CacheCleanRetained {
                relative_path: relative,
                reason: CacheCleanRefusal::UnrecognizedEntry,
                detail: "directory name is not a SHA-256 digest".into(),
            });
            continue;
        }
        // Without a compiled digest there is nothing to compare against, so
        // every revision stays.
        if current.is_empty() || name == current {
            retained.push(CacheCleanRetained {
                relative_path: relative,
                reason: CacheCleanRefusal::CurrentModelDigest,
                detail: "digest matches the model compiled into this executable".into(),
            });
            continue;
        }
        match observe_model_tree_lock(&path) {
            ModelTreeLock::Free => {}
            ModelTreeLock::Held => {
                retained.push(CacheCleanRetained {
                    relative_path: relative,
                    reason: CacheCleanRefusal::ModelTreeInUse,
                    detail: "a peer holds this model tree's materialization lock".into(),
                });
                continue;
            }
            ModelTreeLock::Unobservable(detail) => {
                retained.push(CacheCleanRetained {
                    relative_path: relative,
                    reason: CacheCleanRefusal::UnsafeEntry,
                    detail,
                });
                continue;
            }
        }
        match directory_size(&path) {
            Ok(bytes) => candidates.push(CacheCleanCandidate {
                kind: CacheCleanKind::SupersededModelDigest,
                relative_path: relative,
                bytes,
                proof: format!(
                    "digest differs from the compiled model digest {current} and no peer holds its materialization lock"
                ),
            }),
            Err(error) => {
                errors.push(format!(
                    "measure model digest {}: {error:#}",
                    path.display()
                ));
                retained.push(CacheCleanRetained {
                    relative_path: relative,
                    reason: CacheCleanRefusal::UnsafeEntry,
                    detail: format!("model digest directory could not be measured: {error:#}"),
                });
            }
        }
    }
}

fn is_workspace_cache_name(name: &str) -> bool {
    name.len() == WORKSPACE_ID_HEX_LEN && name.bytes().all(is_lowercase_hex)
}

fn is_model_digest_name(name: &str) -> bool {
    name.len() == MODEL_DIGEST_HEX_LEN && name.bytes().all(is_lowercase_hex)
}

const fn is_lowercase_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SidecarLayout, with_test_cache_root};
    use crate::retention::{GenerationRetentionMarker, write_retention_marker};
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::RetrievalIndexManifest;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use tempfile::{TempDir, tempdir};

    const OTHER_DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn snapshot_tree(root: &Path) -> BTreeMap<String, String> {
        let mut entries = BTreeMap::new();
        collect_tree(root, root, &mut entries);
        entries
    }

    fn collect_tree(root: &Path, dir: &Path, entries: &mut BTreeMap<String, String>) {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("entry is below the snapshot root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("inspect snapshot entry");
            if metadata.is_dir() {
                entries.insert(format!("{relative}/"), "<dir>".to_string());
                collect_tree(root, &path, entries);
            } else {
                let bytes = std::fs::read(&path).expect("read snapshot entry");
                let digest = Sha256::digest(&bytes);
                entries.insert(relative, format!("{}:{digest:x}", bytes.len()));
            }
        }
    }

    fn manifest(project_id: &str, suffix: &str) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: project_id.into(),
            lexical_version: "v1".into(),
            semantic_generation: format!("codestory_{project_id}_{suffix}"),
            scip_revision: Some(format!("graph-{suffix}")),
            built_at_epoch_ms: 1,
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

    fn state_file_for(cache_root: &Path) -> PathBuf {
        local_state_file(cache_root)
    }

    /// A real, current-schema core store holding no authored annotation. The
    /// planner reads this database, so a stand-in byte blob would prove
    /// nothing about the guard under test.
    fn write_derived_core_store(path: &Path) {
        let store = Store::open(path).expect("create core store");
        drop(store);
        assert_eq!(
            observe_legacy_annotation_rows(path).expect("observe a freshly created core store"),
            0
        );
    }

    /// The same core store with a user-authored bookmark in it: a category and
    /// a bookmark on a node, in `bookmark_category` and `bookmark_node`.
    fn author_bookmark_in_core_store(path: &Path) {
        let store = Store::open(path).expect("open core store");
        store
            .insert_node(&Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "n:auth.rs:login".into(),
                qualified_name: Some("auth::login".into()),
                canonical_id: Some("auth::login".into()),
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(0),
                end_line: Some(9),
                end_col: Some(1),
            })
            .expect("insert bookmarked node");
        let category = store
            .create_bookmark_category("Onboarding")
            .expect("authored bookmark category");
        store
            .add_bookmark(category, NodeId(1), Some("start here"))
            .expect("authored bookmark");
        drop(store);
    }

    /// A per-project cache holding a real core store, registered to
    /// `worktree`.
    fn register_project_cache(
        cache_root: &Path,
        workspace_id: &str,
        worktree: &Path,
        suffix: &str,
    ) {
        let dir = cache_root.join(workspace_id);
        std::fs::create_dir_all(&dir).expect("project cache dir");
        write_derived_core_store(&dir.join("codestory.db"));
        let marker = GenerationRetentionMarker::next(
            workspace_id,
            worktree,
            manifest("repo-v1-project", suffix),
            None,
            1,
        )
        .expect("registration marker");
        write_retention_marker(&state_file_for(cache_root), &marker).expect("write marker");
    }

    fn write_model_digest(cache_root: &Path, digest: &str, bytes: usize) {
        let dir = embedded_model_digest_root(cache_root).join(digest);
        std::fs::create_dir_all(&dir).expect("model digest dir");
        std::fs::write(dir.join("model.gguf"), vec![b'm'; bytes]).expect("model bytes");
    }

    fn retained_reason(plan: &CacheCleanPlan, relative: &str) -> Option<CacheCleanRefusal> {
        plan.retained
            .iter()
            .find(|entry| entry.relative_path == relative)
            .map(|entry| entry.reason)
    }

    fn candidate_kinds(plan: &CacheCleanPlan) -> Vec<(String, CacheCleanKind)> {
        plan.candidates
            .iter()
            .map(|candidate| (candidate.relative_path.clone(), candidate.kind))
            .collect()
    }

    struct Fixture {
        cache: TempDir,
        _worktrees: TempDir,
        live_workspace: String,
        dead_workspace: String,
        unregistered_workspace: String,
    }

    /// One cache root holding, side by side: a live registered workspace, a
    /// registered workspace whose worktree was deleted, an unregistered
    /// workspace, the current model digest, and a superseded one.
    fn fixture() -> Fixture {
        let cache = tempdir().expect("cache root");
        let worktrees = tempdir().expect("worktrees");
        let live_root = worktrees.path().join("live");
        let dead_root = worktrees.path().join("dead");
        std::fs::create_dir_all(&live_root).expect("live worktree");
        std::fs::create_dir_all(&dead_root).expect("dead worktree");
        let live_workspace = "aaaa000000000001".to_string();
        let dead_workspace = "bbbb000000000002".to_string();
        let unregistered_workspace = "cccc000000000003".to_string();
        register_project_cache(
            cache.path(),
            &live_workspace,
            &live_root,
            "aaaaaaaaaaaaaaaa",
        );
        register_project_cache(
            cache.path(),
            &dead_workspace,
            &dead_root,
            "bbbbbbbbbbbbbbbb",
        );
        let unregistered = cache.path().join(&unregistered_workspace);
        std::fs::create_dir_all(&unregistered).expect("unregistered cache");
        std::fs::write(unregistered.join("codestory.db"), b"unregistered")
            .expect("unregistered store");
        write_model_digest(cache.path(), codestory_llama_sys::MODEL_SHA256, 8);
        write_model_digest(cache.path(), OTHER_DIGEST, 12);
        std::fs::remove_dir_all(&dead_root).expect("retire the dead worktree");
        Fixture {
            cache,
            _worktrees: worktrees,
            live_workspace,
            dead_workspace,
            unregistered_workspace,
        }
    }

    #[test]
    fn a_dry_run_plan_leaves_the_cache_tree_byte_identical() {
        let fixture = fixture();
        let before = snapshot_tree(fixture.cache.path());

        let plan = with_test_cache_root(fixture.cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert_eq!(
            before,
            snapshot_tree(fixture.cache.path()),
            "planning cleanup must not add, remove, or rewrite one byte"
        );
        assert!(plan.dry_run);
        assert!(plan.reclaimable_bytes > 0);
    }

    #[test]
    fn only_a_retired_registration_and_a_superseded_digest_are_candidates() {
        let fixture = fixture();

        let plan = with_test_cache_root(fixture.cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        let superseded = format!("embedded-models/sha256/{OTHER_DIGEST}");
        assert_eq!(
            candidate_kinds(&plan),
            vec![
                (
                    fixture.dead_workspace.clone(),
                    CacheCleanKind::AbandonedProjectCache
                ),
                (superseded.clone(), CacheCleanKind::SupersededModelDigest),
            ]
        );
        assert_eq!(
            retained_reason(&plan, &fixture.live_workspace),
            Some(CacheCleanRefusal::LiveWorkspace)
        );
        assert_eq!(
            retained_reason(&plan, &fixture.unregistered_workspace),
            Some(CacheCleanRefusal::UnregisteredWorkspace)
        );
        assert_eq!(
            retained_reason(
                &plan,
                &format!(
                    "embedded-models/sha256/{}",
                    codestory_llama_sys::MODEL_SHA256
                )
            ),
            Some(CacheCleanRefusal::CurrentModelDigest)
        );
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    }

    #[test]
    fn apply_removes_the_proven_candidates_and_nothing_else() {
        let fixture = fixture();
        let cache_root = fixture.cache.path().to_path_buf();

        let report = with_test_cache_root(&cache_root, || {
            apply_cache_clean().expect("cache clean apply")
        });

        assert!(!report.dry_run);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            report
                .removals
                .iter()
                .map(|removal| (removal.relative_path.as_str(), removal.removed))
                .collect::<Vec<_>>(),
            vec![
                (fixture.dead_workspace.as_str(), true),
                (
                    format!("embedded-models/sha256/{OTHER_DIGEST}").as_str(),
                    true
                ),
            ]
        );
        assert!(!cache_root.join(&fixture.dead_workspace).exists());
        assert!(
            !embedded_model_digest_root(&cache_root)
                .join(OTHER_DIGEST)
                .exists()
        );
        assert!(cache_root.join(&fixture.live_workspace).is_dir());
        assert!(cache_root.join(&fixture.unregistered_workspace).is_dir());
        assert!(
            embedded_model_digest_root(&cache_root)
                .join(codestory_llama_sys::MODEL_SHA256)
                .is_dir()
        );
    }

    /// Annotations are authored, not derived. A workspace that no longer
    /// exists cannot regenerate them, so proven abandonment is not licence to
    /// take them.
    #[test]
    fn a_retired_cache_holding_annotations_is_not_removed() {
        let fixture = fixture();
        let cache_root = fixture.cache.path().to_path_buf();
        std::fs::write(
            cache_root
                .join(&fixture.dead_workspace)
                .join(ANNOTATIONS_SIDECAR_FILE),
            b"authored annotations",
        )
        .expect("annotations sidecar");

        let report = with_test_cache_root(&cache_root, || {
            apply_cache_clean().expect("cache clean apply")
        });

        assert!(
            cache_root
                .join(&fixture.dead_workspace)
                .join(ANNOTATIONS_SIDECAR_FILE)
                .exists(),
            "cleanup must never take authored annotations"
        );
        assert_eq!(
            retained_reason(&report.plan, &fixture.dead_workspace),
            Some(CacheCleanRefusal::AnnotationsPresent)
        );
        assert_eq!(
            report
                .removals
                .iter()
                .map(|removal| removal.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![format!("embedded-models/sha256/{OTHER_DIGEST}").as_str()]
        );
    }

    /// The regression this guard exists for. Bookmarks are authored in the
    /// core store's own `bookmark_category` and `bookmark_node` tables, and
    /// the annotations sidecar only appears once a cache cuts over. An
    /// abandoned worktree is never re-opened, so it can never cut over: its
    /// cache is exactly the population where the sidecar file is guaranteed
    /// absent while authored bookmarks are present. Testing for the file
    /// deleted them.
    #[test]
    fn a_retired_cache_with_authored_bookmarks_in_core_legacy_tables_is_not_removed() {
        let fixture = fixture();
        let cache_root = fixture.cache.path().to_path_buf();
        let dead_cache = cache_root.join(&fixture.dead_workspace);
        author_bookmark_in_core_store(&dead_cache.join("codestory.db"));
        assert!(
            !dead_cache.join(ANNOTATIONS_SIDECAR_FILE).exists(),
            "the population under test is precisely the one with no sidecar"
        );

        let report = with_test_cache_root(&cache_root, || {
            apply_cache_clean().expect("cache clean apply")
        });

        assert!(
            dead_cache.join("codestory.db").is_file(),
            "cleanup must never take a cache holding authored bookmarks"
        );
        let surviving = Store::open_observational(dead_cache.join("codestory.db"))
            .expect("re-observe the surviving core store");
        assert_eq!(
            surviving
                .get_bookmark_categories()
                .expect("surviving categories")
                .len(),
            1
        );
        assert_eq!(
            surviving.get_bookmarks(None).expect("surviving bookmarks")[0]
                .comment
                .as_deref(),
            Some("start here")
        );
        assert_eq!(
            retained_reason(&report.plan, &fixture.dead_workspace),
            Some(CacheCleanRefusal::AnnotationsPresent)
        );
        assert_eq!(
            report
                .removals
                .iter()
                .map(|removal| removal.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![format!("embedded-models/sha256/{OTHER_DIGEST}").as_str()]
        );
    }

    /// Absence of authored annotations must be read, never inferred. A core
    /// store this binary cannot open proves nothing, so the cache stays.
    #[test]
    fn a_retired_cache_whose_core_store_cannot_be_observed_is_not_removed() {
        let fixture = fixture();
        let cache_root = fixture.cache.path().to_path_buf();
        let dead_cache = cache_root.join(&fixture.dead_workspace);
        std::fs::write(dead_cache.join("codestory.db"), vec![b'x'; 16])
            .expect("unopenable core store");

        let report = with_test_cache_root(&cache_root, || {
            apply_cache_clean().expect("cache clean apply")
        });

        assert!(
            dead_cache.is_dir(),
            "an unreadable cache must never be deleted"
        );
        assert_eq!(
            retained_reason(&report.plan, &fixture.dead_workspace),
            Some(CacheCleanRefusal::AnnotationsUnproven)
        );
        assert_eq!(
            report
                .removals
                .iter()
                .map(|removal| removal.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![format!("embedded-models/sha256/{OTHER_DIGEST}").as_str()]
        );
    }

    /// An orphaned WAL is evidence this reader cannot interpret, not derived
    /// bytes it may drop.
    #[test]
    fn a_retired_cache_holding_an_orphaned_sqlite_sidecar_is_not_removed() {
        let fixture = fixture();
        let cache_root = fixture.cache.path().to_path_buf();
        let dead_cache = cache_root.join(&fixture.dead_workspace);
        std::fs::remove_file(dead_cache.join("codestory.db")).expect("drop the core store");
        std::fs::write(dead_cache.join("codestory.db-wal"), b"orphan frames")
            .expect("orphaned write-ahead log");

        let plan = with_test_cache_root(&cache_root, || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert_eq!(
            retained_reason(&plan, &fixture.dead_workspace),
            Some(CacheCleanRefusal::AnnotationsUnproven)
        );
    }

    /// A model tree a peer is materializing right now is not reclaimable, and
    /// observing that must not create the lock file.
    #[test]
    fn a_superseded_model_tree_whose_materialization_lock_is_held_is_not_removed() {
        let fixture = fixture();
        let cache_root = fixture.cache.path().to_path_buf();
        let superseded = embedded_model_digest_root(&cache_root).join(OTHER_DIGEST);
        let lock_path = superseded.join(EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE);
        std::fs::write(&lock_path, b"").expect("materialization lock file");
        let holder = File::options()
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open the materialization lock");
        assert!(
            bounded_locks::try_acquire(&holder, FileLockKind::Exclusive)
                .expect("hold the materialization lock"),
            "the peer must actually hold the lock"
        );

        let report = with_test_cache_root(&cache_root, || {
            apply_cache_clean().expect("cache clean apply")
        });

        assert!(
            superseded.is_dir(),
            "a model tree a peer is materializing must not be reclaimed"
        );
        assert_eq!(
            retained_reason(
                &report.plan,
                &format!("embedded-models/sha256/{OTHER_DIGEST}")
            ),
            Some(CacheCleanRefusal::ModelTreeInUse)
        );
        assert_eq!(
            report
                .removals
                .iter()
                .map(|removal| removal.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec![fixture.dead_workspace.as_str()]
        );
        bounded_locks::release(&holder).expect("release the materialization lock");
    }

    /// Observation never creates the lock file a dry run only wanted to read.
    #[test]
    fn planning_does_not_create_a_model_materialization_lock() {
        let cache = tempdir().expect("cache root");
        write_model_digest(cache.path(), OTHER_DIGEST, 12);
        let lock_path = embedded_model_digest_root(cache.path())
            .join(OTHER_DIGEST)
            .join(EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE);

        let plan = with_test_cache_root(cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert!(
            !lock_path.exists(),
            "a dry-run plan must not be the reason the lock file exists"
        );
        assert_eq!(
            candidate_kinds(&plan),
            vec![(
                format!("embedded-models/sha256/{OTHER_DIGEST}"),
                CacheCleanKind::SupersededModelDigest
            )]
        );
    }

    /// A schema-1 marker registers nothing, so its cache can never be proven
    /// abandoned no matter how long the worktree has been gone.
    #[test]
    fn a_schema_1_registration_never_makes_a_cache_removable() {
        let cache = tempdir().expect("cache root");
        let workspace_id = "dddd000000000004";
        let dir = cache.path().join(workspace_id);
        std::fs::create_dir_all(&dir).expect("project cache dir");
        std::fs::write(dir.join("codestory.db"), b"legacy").expect("store");
        let legacy = GenerationRetentionMarker {
            schema_version: crate::retention::RETENTION_MARKER_SCHEMA_V1,
            workspace_id: workspace_id.into(),
            project_id: "repo-v1-project".into(),
            active: manifest("repo-v1-project", "aaaaaaaaaaaaaaaa"),
            rollback: None,
            updated_at_epoch_ms: 1,
            owner: None,
            generation: None,
            heartbeat_epoch_ms: None,
        };
        let state_file = state_file_for(cache.path());
        crate::retention::ensure_retention_dir(&state_file).expect("retention dir");
        std::fs::write(
            crate::retention::retention_marker_path(&state_file, workspace_id)
                .expect("marker path"),
            serde_json::to_vec(&legacy).expect("serialize legacy marker"),
        )
        .expect("write legacy marker");

        let plan = with_test_cache_root(cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert!(candidate_kinds(&plan).is_empty(), "{:?}", plan.candidates);
        assert_eq!(
            retained_reason(&plan, workspace_id),
            Some(CacheCleanRefusal::UnregisteredWorkspace)
        );
    }

    /// An unplugged drive or unmounted share makes the whole ancestor path
    /// disappear, which looks exactly like a deleted worktree if only the leaf
    /// is checked. An absent volume is not evidence of abandonment.
    #[test]
    fn a_registration_whose_whole_ancestor_path_vanished_is_not_proven_abandoned() {
        let cache = tempdir().expect("cache root");
        let mount = tempdir().expect("mount point");
        let volume = mount.path().join("volume");
        let worktree = volume.join("project");
        std::fs::create_dir_all(&worktree).expect("worktree on the volume");
        let workspace_id = "eeee000000000005";
        register_project_cache(cache.path(), workspace_id, &worktree, "aaaaaaaaaaaaaaaa");
        std::fs::remove_dir_all(&volume).expect("unmount the volume");

        let plan = with_test_cache_root(cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert!(candidate_kinds(&plan).is_empty(), "{:?}", plan.candidates);
        assert_eq!(
            retained_reason(&plan, workspace_id),
            Some(CacheCleanRefusal::UnprovenWorkspace)
        );
    }

    #[test]
    fn a_non_digest_directory_under_the_model_root_is_never_a_candidate() {
        let cache = tempdir().expect("cache root");
        let stray = embedded_model_digest_root(cache.path()).join("scratch");
        std::fs::create_dir_all(&stray).expect("stray dir");
        std::fs::write(stray.join("note"), b"not a digest").expect("stray file");

        let plan = with_test_cache_root(cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert!(candidate_kinds(&plan).is_empty(), "{:?}", plan.candidates);
        assert_eq!(
            retained_reason(&plan, "embedded-models/sha256/scratch"),
            Some(CacheCleanRefusal::UnrecognizedEntry)
        );
    }

    /// The sidecar artifact roots share the cache root with per-project
    /// caches. Only the 16-hex workspace shape may ever be considered.
    #[test]
    fn sidecar_artifact_roots_are_not_project_cache_candidates() {
        let cache = tempdir().expect("cache root");
        let layout = SidecarLayout {
            lexical_data_dir: cache.path().join("lexical"),
            semantic_data_dir: cache.path().join("semantic"),
            scip_artifacts_root: cache.path().join("scip"),
            state_file: state_file_for(cache.path()),
        };
        for dir in [
            &layout.lexical_data_dir,
            &layout.semantic_data_dir,
            &layout.scip_artifacts_root,
            &cache.path().join("retrieval"),
        ] {
            std::fs::create_dir_all(dir).expect("artifact root");
            std::fs::write(dir.join("data"), b"artifact").expect("artifact bytes");
        }

        let plan = with_test_cache_root(cache.path(), || {
            plan_cache_clean().expect("cache clean plan")
        });

        assert!(candidate_kinds(&plan).is_empty(), "{:?}", plan.candidates);
        assert!(plan.retained.is_empty(), "{:?}", plan.retained);
    }
}
