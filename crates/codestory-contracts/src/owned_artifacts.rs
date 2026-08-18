//! One registry of every file identity CodeStory writes beside its core
//! storage file.
//!
//! Producers derive their artifact paths from these constants and functions,
//! and the workspace discovery exclusion consumes the same registry, so an
//! artifact name exists in exactly one place. The architecture contract test
//! rejects production sources that spell these identities directly, which is
//! what keeps a future owned artifact from silently entering source discovery
//! the way an unregistered one would.

use std::path::{Path, PathBuf};

/// SQLite sidecar suffixes appended to a live database file name.
pub const SQLITE_SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Extension replacing the storage extension for the rollback backup.
pub const ROLLBACK_BACKUP_EXTENSION: &str = "sqlite.backup";

/// Extension replacing the storage extension for the index-writer lock.
pub const INDEX_WRITER_LOCK_EXTENSION: &str = "index-writer.lock";

/// Suffixes appended to the full storage file name by staged promotion.
pub const PROMOTION_LOCK_SUFFIX: &str = ".promotion.lock";
pub const PROMOTION_PREPARED_JOURNAL_SUFFIX: &str = ".promotion.prepared.json";
pub const PROMOTION_COMMITTED_JOURNAL_SUFFIX: &str = ".promotion.committed.json";
pub const PROMOTION_CLEANUP_BLOCKED_SUFFIX: &str = ".promotion.cleanup-blocked";

/// Every promotion sibling suffix, in stable order.
pub const PROMOTION_SIBLING_SUFFIXES: [&str; 4] = [
    PROMOTION_LOCK_SUFFIX,
    PROMOTION_PREPARED_JOURNAL_SUFFIX,
    PROMOTION_COMMITTED_JOURNAL_SUFFIX,
    PROMOTION_CLEANUP_BLOCKED_SUFFIX,
];

/// Infix between the storage stem and the `{pid}-{epoch}` unique part of a
/// staged snapshot name.
pub const STAGED_SNAPSHOT_INFIX: &str = ".staged.";

/// Directory-name suffixes for the search trees owned by one storage file,
/// applied to the storage stem. Each directory also owns a sibling
/// `<directory>.lock` file.
pub const SEARCH_DIRECTORY_SUFFIXES: [&str; 2] = ["search", "search-generations"];

/// Local-refresh serialization files written by the CLI into the cache root
/// that holds the storage file. The state guard is persistent by design.
pub const LOCAL_REFRESH_STATUS_FILE: &str = "local-refresh-status.json";
pub const LOCAL_REFRESH_LOCK_FILE: &str = "local-refresh.lock";
pub const LOCAL_REFRESH_STATE_GUARD_FILE: &str = "local-refresh-state.guard";

/// Annotations sidecar beside the storage file. Registered ahead of the
/// sidecar cutover so discovery excludes it from the first write.
pub const ANNOTATIONS_SIDECAR_FILE: &str = "annotations.sqlite3";

/// Retained pre-migration export of the core annotation tables, written once
/// by the sidecar cutover and kept for the documented downgrade path.
pub const ANNOTATIONS_MIGRATION_BACKUP_FILE: &str = "annotations.pre-migration.json";

/// Fixed file names CodeStory owns inside the cache root that holds the
/// storage file, independent of the storage file's own name.
pub const CACHE_ROOT_OWNED_FILE_NAMES: [&str; 3] = [
    LOCAL_REFRESH_STATUS_FILE,
    LOCAL_REFRESH_LOCK_FILE,
    LOCAL_REFRESH_STATE_GUARD_FILE,
];

/// Content-addressed materialization tree for the embedded embedding model,
/// rooted at the process cache root rather than beside any storage file.
///
/// The digest segment names one immutable model revision, so a cleanup pass
/// can only prove a sibling superseded by comparing it against the digest the
/// running executable was compiled with. Both segments live here so the
/// producer and the cleanup planner cannot drift.
pub const EMBEDDED_MODEL_CACHE_DIR: &str = "embedded-models";
pub const EMBEDDED_MODEL_DIGEST_DIR: &str = "sha256";
/// Advisory lock beside one materialized model revision.
pub const EMBEDDED_MODEL_MATERIALIZE_LOCK_FILE: &str = ".materialize.lock";

/// Root holding one directory per content-addressed model digest.
pub fn embedded_model_digest_root(cache_root: &Path) -> PathBuf {
    cache_root
        .join(EMBEDDED_MODEL_CACHE_DIR)
        .join(EMBEDDED_MODEL_DIGEST_DIR)
}

/// Directory holding the materialized model for one digest.
pub fn embedded_model_directory(cache_root: &Path, digest: &str) -> PathBuf {
    embedded_model_digest_root(cache_root).join(digest)
}

/// Directory the guided derived-cache reset moves quarantined derived state
/// into, inside the cache root that holds the storage file. The reset moves
/// rather than deletes, so the name is an owned identity like any other.
pub const DERIVED_RESET_QUARANTINE_DIR: &str = "derived-reset-quarantine";

fn cache_root_for(storage_path: &Path) -> &Path {
    storage_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn storage_stem(storage_path: &Path) -> &str {
    storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("codestory")
}

fn storage_extension(storage_path: &Path) -> &str {
    storage_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("sqlite")
}

fn path_with_display_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

fn path_with_native_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut suffixed = path.as_os_str().to_os_string();
    suffixed.push(suffix);
    PathBuf::from(suffixed)
}

/// The live file plus its SQLite sidecars.
pub fn sqlite_file_with_sidecars(live_path: &Path) -> Vec<PathBuf> {
    let mut files = vec![live_path.to_path_buf()];
    files.extend(
        SQLITE_SIDECAR_SUFFIXES
            .iter()
            .map(|suffix| path_with_native_suffix(live_path, suffix)),
    );
    files
}

/// Search directory owned by one storage file for the given registry suffix.
pub fn search_directory_for_storage(storage_path: &Path, suffix: &str) -> PathBuf {
    cache_root_for(storage_path).join(format!("{}.{suffix}", storage_stem(storage_path)))
}

/// Annotations sidecar beside one storage file.
///
/// The sidecar sits in the cache root next to the storage file but outside the
/// core promotion fence, so its name is derived from the cache root rather than
/// from the storage stem.
pub fn annotations_sidecar_path(storage_path: &Path) -> PathBuf {
    cache_root_for(storage_path).join(ANNOTATIONS_SIDECAR_FILE)
}

/// Retained pre-migration annotation export beside one storage file.
pub fn annotations_migration_backup_path(storage_path: &Path) -> PathBuf {
    cache_root_for(storage_path).join(ANNOTATIONS_MIGRATION_BACKUP_FILE)
}

/// The index-writer lock one storage file's indexing runs hold end to end.
pub fn index_writer_lock_path(storage_path: &Path) -> PathBuf {
    storage_path.with_extension(INDEX_WRITER_LOCK_EXTENSION)
}

/// Exact owned file identities beside one storage file: the database and its
/// sidecars, the index-writer lock, promotion siblings, the rollback backup
/// and its sidecars, the search directory locks, the local-refresh files, and
/// the annotations sidecar with its sidecars.
pub fn storage_owned_file_identities(storage_path: &Path) -> Vec<PathBuf> {
    let cache_root = cache_root_for(storage_path);
    let rollback_backup = storage_path.with_extension(ROLLBACK_BACKUP_EXTENSION);
    let mut files = sqlite_file_with_sidecars(storage_path);
    files.push(index_writer_lock_path(storage_path));
    files.extend(
        PROMOTION_SIBLING_SUFFIXES
            .iter()
            .map(|suffix| path_with_display_suffix(storage_path, suffix)),
    );
    files.extend(sqlite_file_with_sidecars(&rollback_backup));
    files.extend(SEARCH_DIRECTORY_SUFFIXES.iter().map(|suffix| {
        path_with_native_suffix(&search_directory_for_storage(storage_path, suffix), ".lock")
    }));
    files.extend(
        CACHE_ROOT_OWNED_FILE_NAMES
            .iter()
            .map(|name| cache_root.join(name)),
    );
    files.extend(sqlite_file_with_sidecars(&annotations_sidecar_path(
        storage_path,
    )));
    files.push(annotations_migration_backup_path(storage_path));
    files
}

/// One promotion sibling path for a storage file.
pub fn promotion_sibling_path(storage_path: &Path, suffix: &str) -> PathBuf {
    path_with_display_suffix(storage_path, suffix)
}

/// The annotations sidecar and its SQLite siblings in one cache root.
///
/// These hold user-authored state. Nothing that reclaims derived output may
/// move or remove them, which is why they are named separately from the rest
/// of the owned set rather than filtered at each call site.
pub fn annotation_owned_file_identities(cache_root: &Path) -> Vec<PathBuf> {
    sqlite_file_with_sidecars(&cache_root.join(ANNOTATIONS_SIDECAR_FILE))
}

/// Root the guided derived-cache reset quarantines into for one storage file.
pub fn derived_reset_quarantine_root(storage_path: &Path) -> PathBuf {
    cache_root_for(storage_path).join(DERIVED_RESET_QUARANTINE_DIR)
}

/// Exclusions the guided reset holds for the whole move, in acquisition order.
///
/// An indexing run holds the index-writer lock for its entire pass and takes
/// the promotion lock inside it, so the reset must take them in the same order
/// or the two paths can deadlock against each other. The promotion lock alone
/// excludes only the publish critical section, which is a few milliseconds of
/// a minutes-long index run — a reset that took only that lock would move the
/// cache out from under a live indexer.
pub fn derived_reset_held_lock_paths(storage_path: &Path) -> [PathBuf; 2] {
    [
        index_writer_lock_path(storage_path),
        promotion_sibling_path(storage_path, PROMOTION_LOCK_SUFFIX),
    ]
}

/// Derived file identities the guided reset quarantines.
///
/// This is every file identity the storage file owns, minus two exclusion
/// families. The annotation sidecar family is user-owned state and is
/// preserved in place. The locks in [`derived_reset_held_lock_paths`] are the
/// exclusions the reset itself holds while it runs, and an empty coordination
/// file carries no derived state, so moving one would only drop the exclusion
/// a concurrent publisher or indexer is waiting on — or let a new indexer take
/// a fresh lock file at the old path and start writing mid-reset.
pub fn derived_reset_file_identities(storage_path: &Path) -> Vec<PathBuf> {
    let preserved = annotation_owned_file_identities(cache_root_for(storage_path));
    let held = derived_reset_held_lock_paths(storage_path);
    storage_owned_file_identities(storage_path)
        .into_iter()
        .filter(|path| !held.contains(path) && !preserved.contains(path))
        .collect()
}

/// Derived directory identities the guided reset quarantines: the search trees
/// owned by one storage file.
pub fn derived_reset_directory_identities(storage_path: &Path) -> Vec<PathBuf> {
    SEARCH_DIRECTORY_SUFFIXES
        .iter()
        .map(|suffix| search_directory_for_storage(storage_path, suffix))
        .collect()
}

/// Build the staged snapshot path for one live database and unique parts.
pub fn staged_snapshot_path(live_path: &Path, pid: u32, epoch_ns: u128) -> PathBuf {
    cache_root_for(live_path).join(format!(
        "{}{}{pid}-{epoch_ns}.{}",
        storage_stem(live_path),
        STAGED_SNAPSHOT_INFIX,
        storage_extension(live_path)
    ))
}

/// Whether one file name belongs to the staged snapshot namespace of the
/// given storage file: `{stem}.staged.{pid}-{epoch}.{ext}[-wal|-shm|-journal]`.
pub fn is_staged_snapshot_name(storage_path: &Path, file_name: &str) -> bool {
    let prefix = format!("{}{}", storage_stem(storage_path), STAGED_SNAPSHOT_INFIX);
    let extension = storage_extension(storage_path);
    let Some(candidate) = file_name.strip_prefix(&prefix) else {
        return false;
    };
    let mut suffixes = vec![format!(".{extension}")];
    suffixes.extend(
        SQLITE_SIDECAR_SUFFIXES
            .iter()
            .map(|sidecar| format!(".{extension}{sidecar}")),
    );
    let Some(unique) = suffixes
        .iter()
        .find_map(|suffix| candidate.strip_suffix(suffix.as_str()))
    else {
        return false;
    };
    let mut unique_parts = unique.split('-');
    unique_parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && unique_parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && unique_parts.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_identities_cover_every_registered_family() {
        let storage = Path::new("/cache/custom-core.db");
        let files = storage_owned_file_identities(storage);
        let expect = |name: &str| {
            assert!(
                files.iter().any(|file| file == Path::new(name)),
                "{name} must be registry-owned"
            );
        };
        expect("/cache/custom-core.db");
        expect("/cache/custom-core.db-wal");
        expect("/cache/custom-core.index-writer.lock");
        expect("/cache/custom-core.db.promotion.lock");
        expect("/cache/custom-core.db.promotion.cleanup-blocked");
        expect("/cache/custom-core.sqlite.backup");
        expect("/cache/custom-core.sqlite.backup-journal");
        expect("/cache/custom-core.search.lock");
        expect("/cache/custom-core.search-generations.lock");
        expect("/cache/local-refresh-status.json");
        expect("/cache/local-refresh.lock");
        expect("/cache/local-refresh-state.guard");
        expect("/cache/annotations.sqlite3");
        expect("/cache/annotations.sqlite3-shm");
        expect("/cache/annotations.pre-migration.json");
    }

    #[test]
    fn derived_reset_quarantines_the_core_cache_and_preserves_annotations() {
        let storage = Path::new("/cache/custom-core.db");
        let derived = derived_reset_file_identities(storage);
        let contains = |name: &str| derived.iter().any(|file| file == Path::new(name));

        for annotation in annotation_owned_file_identities(Path::new("/cache")) {
            assert!(
                !derived.contains(&annotation),
                "{} is user-owned annotation state and must never be quarantined",
                annotation.display()
            );
        }
        assert_eq!(
            derived_reset_held_lock_paths(storage),
            [
                PathBuf::from("/cache/custom-core.index-writer.lock"),
                PathBuf::from("/cache/custom-core.db.promotion.lock"),
            ],
            "the reset must exclude an indexing run, not only a publish, and must take the two locks in the order an indexer takes them"
        );
        for held in derived_reset_held_lock_paths(storage) {
            assert!(
                !derived.contains(&held),
                "{} is the exclusion the reset holds; quarantining it would let a peer take a fresh lock at the live path mid-reset",
                held.display()
            );
        }
        for required in [
            "/cache/custom-core.db",
            "/cache/custom-core.db-wal",
            "/cache/custom-core.db-shm",
            "/cache/custom-core.db.promotion.prepared.json",
            "/cache/custom-core.db.promotion.committed.json",
            "/cache/custom-core.db.promotion.cleanup-blocked",
            "/cache/custom-core.sqlite.backup",
            "/cache/custom-core.search.lock",
            "/cache/custom-core.search-generations.lock",
            "/cache/local-refresh-status.json",
            "/cache/local-refresh.lock",
            "/cache/local-refresh-state.guard",
        ] {
            assert!(
                contains(required),
                "{required} must be quarantined by the reset"
            );
        }
        assert_eq!(
            derived_reset_directory_identities(storage),
            vec![
                PathBuf::from("/cache/custom-core.search"),
                PathBuf::from("/cache/custom-core.search-generations"),
            ]
        );
        assert_eq!(
            derived_reset_quarantine_root(storage),
            Path::new("/cache").join(DERIVED_RESET_QUARANTINE_DIR)
        );
    }

    #[test]
    fn staged_namespace_matches_only_pid_epoch_names() {
        let storage = Path::new("/cache/custom-core.db");
        let staged = staged_snapshot_path(storage, 123, 456);
        assert_eq!(staged, Path::new("/cache/custom-core.staged.123-456.db"));
        assert!(is_staged_snapshot_name(
            storage,
            "custom-core.staged.123-456.db"
        ));
        assert!(is_staged_snapshot_name(
            storage,
            "custom-core.staged.123-456.db-wal"
        ));
        assert!(!is_staged_snapshot_name(
            storage,
            "custom-core.staged.notes.db"
        ));
        assert!(!is_staged_snapshot_name(
            storage,
            "custom-core.staged.123-456-789.db"
        ));
        assert!(!is_staged_snapshot_name(storage, "other.staged.123-456.db"));
    }

    #[test]
    fn bare_storage_names_observe_the_current_directory() {
        let files = storage_owned_file_identities(Path::new("codestory.db"));
        assert!(
            files
                .iter()
                .any(|file| file == Path::new("./local-refresh.lock"))
        );
        assert_eq!(
            staged_snapshot_path(Path::new("codestory.db"), 1, 2),
            Path::new("./codestory.staged.1-2.db")
        );
    }
}
