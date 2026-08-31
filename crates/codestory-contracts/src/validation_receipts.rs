//! Sealed validation receipts.
//!
//! A receipt records that one expensive validation of an *immutable* artifact
//! succeeded. It is only reusable while the native identity of every file the
//! validation examined is the same observation it was sealed to: same presence,
//! same length, same modification and inode-change instants, same device/inode,
//! same read-only bit. Where the platform reports all of that — see the limit
//! below — replacement, truncation, in-place rewriting, and the appearance of a
//! SQLite sidecar all break the seal, so a generation damaged after its first
//! validation re-validates instead of hiding behind the earlier verdict.
//!
//! **A seal is only as strong as the metadata the platform reports, and one
//! shipped platform reports less.** The paragraph above holds in full on Unix,
//! where `std::fs` exposes a device/inode pair and an inode-change instant.
//! Windows exposes neither through `std::fs`, so a seal taken there compares
//! presence, length, the creation and modification instants, and the read-only
//! bit — nothing that records *that the bytes were rewritten*. A writer that
//! rewrites an artifact in place without changing its length and then restores
//! the modification time produces an observation identical to the sealed one,
//! and the receipt answers for bytes it never read. The same is true of a
//! replacement whose length, creation, and modification instants all match.
//! [`SealFidelity`] names which of the two a given observation is, and
//! [`ArtifactSeal::fidelity`] reports it. This is a stated limit of the
//! receipt, not an accident of it: on a
//! [`SealFidelity::TimestampsOnly`] platform a receipt proves the artifact was
//! not casually touched; it does not prove the bytes are the ones the
//! validation read. Nothing that must detect deliberate corruption may rest on
//! a receipt alone there. Concretely, a consumer whose verdict is receipted
//! carries the limit forward: on a timestamps-only platform an artifact
//! rewritten in place at the same length, with its modification time restored,
//! keeps answering with the verdict this process already sealed for it. What
//! bounds that is the receipt's process-local lifetime, not the seal — the next
//! process re-reads the bytes.
//!
//! Two rules are structural rather than conventional:
//!
//! * **Success only.** A failing validation never produces a receipt, and it
//!   removes any receipt the key already had.
//! * **Facts, not verdicts.** A receipt caches what the artifact *is* — the
//!   content-derived value the validation computed. Admission decisions that
//!   depend on caller expectations stay outside the receipt and re-run every
//!   time.
//!
//! Receipts are process-local. They are an optimization over re-reading bytes
//! this process already read; they are never persisted and never shared.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sentinel for a native timestamp the platform does not report.
///
/// A fixed sentinel keeps seal comparison total: two observations of the same
/// file on the same platform always agree, and a platform that gains support
/// mid-process simply invalidates once.
const TIMESTAMP_UNAVAILABLE: i128 = i128::MIN;

/// Why an artifact could not be sealed.
///
/// An unsealable artifact is not an error for the caller: the validation still
/// runs, it just cannot be turned into a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactSealError {
    /// The path exists but is not a regular file.
    NotRegularFile { path: PathBuf },
    /// Native metadata could not be read.
    Unreadable { path: PathBuf, detail: String },
}

impl ArtifactSealError {
    /// Stable machine code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRegularFile { .. } => "artifact_seal_not_regular_file",
            Self::Unreadable { .. } => "artifact_seal_unreadable",
        }
    }
}

impl fmt::Display for ArtifactSealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegularFile { path } => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::Unreadable { path, detail } => {
                write!(
                    formatter,
                    "native metadata for {} is unreadable: {detail}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ArtifactSealError {}

/// How much a present artifact's seal can distinguish.
///
/// This is a property of the observation, not of the caller: it is read off the
/// native metadata the platform actually reported for that file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealFidelity {
    /// The observation carries a device/inode pair and an inode-change instant.
    ///
    /// A rewrite in place breaks the seal even when the writer keeps the length
    /// and restores the modification time afterwards, and a replacement breaks
    /// it even when the replacement's bytes and timestamps match. Unix reports
    /// this.
    InodeChangeTracked,
    /// The observation carries only presence, length, the creation and
    /// modification instants, and the read-only bit.
    ///
    /// Windows is this platform: `std::fs` reports no device/inode pair and no
    /// inode-change instant there, so nothing in the seal records that an
    /// artifact's bytes were rewritten. A same-length rewrite in place that
    /// restores the modification time is indistinguishable from the sealed
    /// observation, and so is a replacement that matches every field. The
    /// residual guarantee is real but narrower: a change in presence, length,
    /// creation instant, modification instant, or the read-only bit still
    /// breaks the seal.
    TimestampsOnly,
}

/// Native identity of one artifact at a single observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSeal {
    path: PathBuf,
    presence: SealPresence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SealPresence {
    /// The artifact did not exist. Absence is part of the seal: a sidecar that
    /// appears after validation invalidates the receipt.
    Absent,
    Present {
        len: u64,
        modified_nanos: i128,
        created_nanos: i128,
        /// Unix device id, `0` where the platform does not report one.
        device: u64,
        /// Unix inode number, `0` where the platform does not report one.
        inode: u64,
        /// Unix inode-change instant. This is what catches an in-place rewrite
        /// that restores the modification time afterwards.
        inode_change_nanos: i128,
        readonly: bool,
    },
}

impl ArtifactSeal {
    /// Observe the native identity of `path` right now.
    ///
    /// A missing path seals as absent. Anything that exists but is not a
    /// regular file — a directory, a symlink standing in for the artifact, a
    /// device node — is refused rather than sealed.
    pub fn observe(path: &Path) -> Result<Self, ArtifactSealError> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(Self {
                path: path.to_path_buf(),
                presence: SealPresence::Present {
                    len: metadata.len(),
                    modified_nanos: metadata
                        .modified()
                        .map_or(TIMESTAMP_UNAVAILABLE, system_time_nanos),
                    created_nanos: metadata
                        .created()
                        .map_or(TIMESTAMP_UNAVAILABLE, system_time_nanos),
                    device: native_device(&metadata),
                    inode: native_inode(&metadata),
                    inode_change_nanos: native_inode_change_nanos(&metadata),
                    readonly: metadata.permissions().readonly(),
                },
            }),
            Ok(_) => Err(ArtifactSealError::NotRegularFile {
                path: path.to_path_buf(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                path: path.to_path_buf(),
                presence: SealPresence::Absent,
            }),
            Err(error) => Err(ArtifactSealError::Unreadable {
                path: path.to_path_buf(),
                detail: error.to_string(),
            }),
        }
    }

    /// The artifact this seal describes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the artifact existed as a regular file when it was sealed.
    pub fn is_present(&self) -> bool {
        matches!(self.presence, SealPresence::Present { .. })
    }

    /// How much this observation can distinguish, or `None` when the artifact
    /// was absent.
    ///
    /// Absence has no weaker form — an absent artifact that reappears always
    /// breaks the seal — so the question only applies to a present one.
    pub fn fidelity(&self) -> Option<SealFidelity> {
        match self.presence {
            SealPresence::Absent => None,
            SealPresence::Present {
                inode,
                inode_change_nanos,
                ..
            } => Some(
                if inode != 0 && inode_change_nanos != TIMESTAMP_UNAVAILABLE {
                    SealFidelity::InodeChangeTracked
                } else {
                    SealFidelity::TimestampsOnly
                },
            ),
        }
    }

    /// Observe every artifact in order, or report the first that cannot be
    /// sealed.
    pub fn observe_all(paths: &[PathBuf]) -> Result<Vec<Self>, ArtifactSealError> {
        paths.iter().map(|path| Self::observe(path)).collect()
    }

    /// Whether this pre-link observation still describes the source after an
    /// owned hard-link operation.
    ///
    /// Creating a hard link changes the inode-change instant on Unix even
    /// though it cannot change the file bytes. Every other observable field,
    /// including the native file identity where available, must remain fixed.
    fn same_source_after_hard_link(&self, after: &Self) -> bool {
        if self.path != after.path {
            return false;
        }
        match (self.presence, after.presence) {
            (SealPresence::Absent, SealPresence::Absent) => true,
            (
                SealPresence::Present {
                    len,
                    modified_nanos,
                    created_nanos,
                    device,
                    inode,
                    readonly,
                    ..
                },
                SealPresence::Present {
                    len: after_len,
                    modified_nanos: after_modified_nanos,
                    created_nanos: after_created_nanos,
                    device: after_device,
                    inode: after_inode,
                    readonly: after_readonly,
                    ..
                },
            ) => {
                len == after_len
                    && modified_nanos == after_modified_nanos
                    && created_nanos == after_created_nanos
                    && native_identity_matches(device, inode, after_device, after_inode)
                    && readonly == after_readonly
            }
            _ => false,
        }
    }

    /// Whether two post-link paths are observations of the same hard-linked
    /// state. The caller has already established the native hard-link relation;
    /// this comparison proves the artifact metadata did not drift around it.
    fn same_hard_link_state(&self, destination: &Self) -> bool {
        match (self.presence, destination.presence) {
            (SealPresence::Absent, SealPresence::Absent) => true,
            (SealPresence::Present { .. }, SealPresence::Present { .. }) => {
                self.presence == destination.presence
            }
            _ => false,
        }
    }
}

fn native_identity_matches(
    left_device: u64,
    left_inode: u64,
    right_device: u64,
    right_inode: u64,
) -> bool {
    if left_device == 0 && left_inode == 0 && right_device == 0 && right_inode == 0 {
        // The hard-link creator must independently prove identity on platforms
        // where std::fs exposes only timestamps. Metadata equality still fences
        // drift before and after that owned operation.
        true
    } else {
        left_device == right_device && left_inode == right_inode
    }
}

fn system_time_nanos(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}

#[cfg(unix)]
fn native_device(metadata: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::dev(metadata)
}

#[cfg(not(unix))]
fn native_device(_metadata: &std::fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn native_inode(metadata: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(metadata)
}

#[cfg(not(unix))]
fn native_inode(_metadata: &std::fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn native_inode_change_nanos(metadata: &std::fs::Metadata) -> i128 {
    use std::os::unix::fs::MetadataExt;
    i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec())
}

#[cfg(not(unix))]
fn native_inode_change_nanos(_metadata: &std::fs::Metadata) -> i128 {
    TIMESTAMP_UNAVAILABLE
}

/// Per-key receipt accounting, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReceiptStats {
    /// Successful validations that produced a receipt for this key.
    pub validations: u64,
    /// Times the sealed receipt answered instead of re-validating.
    pub reuses: u64,
    /// Times a stored receipt was discarded because its seal no longer held.
    pub invalidations: u64,
}

struct Receipt<V> {
    seals: Vec<ArtifactSeal>,
    value: V,
    stats: ReceiptStats,
}

/// A short-lived copy of a valid sealed receipt captured immediately before an
/// owned hard-link operation.
///
/// Callers cannot manufacture one. Installing it under another key still
/// requires both the original and destination artifact sets to prove the
/// expected post-link state.
#[derive(Clone)]
pub struct TransferableReceipt<V> {
    seals: Vec<ArtifactSeal>,
    value: V,
    stats: ReceiptStats,
}

/// A bounded, process-local cache of sealed validation receipts.
///
/// Construct one `static` per validation family. `capacity` bounds the number
/// of live receipts; reaching it clears the cache rather than guessing which
/// generation is still interesting, which keeps the memory bound hard and the
/// eviction rule deterministic.
pub struct SealedReceiptCache<K, V> {
    capacity: usize,
    entries: OnceLock<Mutex<HashMap<K, Receipt<V>>>>,
}

impl<K, V> SealedReceiptCache<K, V> {
    /// A cache holding at most `capacity` receipts.
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: OnceLock::new(),
        }
    }
}

impl<K, V> SealedReceiptCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    fn entries(&self) -> &Mutex<HashMap<K, Receipt<V>>> {
        self.entries.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Answer from a sealed receipt when one still holds, otherwise run
    /// `validate` and seal its success.
    ///
    /// `artifacts` is the complete set of files the verdict depends on. The
    /// seal is taken before validation and re-taken after it; a file that
    /// changed while the validation ran is never sealed, so a receipt can only
    /// describe a state the validation actually observed end to end.
    ///
    /// A failed validation removes any receipt for `key` and is never cached.
    ///
    /// "Still holds" means what [`SealFidelity`] says it means on this
    /// platform. Where the observation is
    /// [`SealFidelity::TimestampsOnly`] — Windows — a same-length in-place
    /// rewrite that restores the modification time still satisfies the seal and
    /// is answered from the receipt.
    pub fn validate_sealed<E>(
        &self,
        key: K,
        artifacts: &[PathBuf],
        validate: impl FnOnce() -> Result<V, E>,
    ) -> Result<V, E> {
        let observed = ArtifactSeal::observe_all(artifacts).ok();
        let mut carried = ReceiptStats::default();
        if let Some(observed) = observed.as_ref() {
            let mut entries = self.locked_entries();
            if let Some(receipt) = entries.get_mut(&key) {
                if &receipt.seals == observed {
                    receipt.stats.reuses += 1;
                    return Ok(receipt.value.clone());
                }
                carried = receipt.stats;
                carried.invalidations += 1;
                // Fail closed: a receipt whose seal no longer holds is gone
                // before the replacement verdict exists.
                entries.remove(&key);
            }
        }

        let value = match validate() {
            Ok(value) => value,
            Err(error) => {
                self.locked_entries().remove(&key);
                return Err(error);
            }
        };

        if let Some(observed) = observed
            && ArtifactSeal::observe_all(artifacts).is_ok_and(|after| after == observed)
        {
            carried.validations += 1;
            let mut entries = self.locked_entries();
            if entries.len() >= self.capacity && !entries.contains_key(&key) {
                entries.clear();
            }
            entries.insert(
                key,
                Receipt {
                    seals: observed,
                    value: value.clone(),
                    stats: carried,
                },
            );
        }
        Ok(value)
    }

    /// Capture a currently valid receipt before creating hard links for an
    /// equivalent immutable generation.
    pub fn transferable_receipt(
        &self,
        key: &K,
        artifacts: &[PathBuf],
    ) -> Option<TransferableReceipt<V>> {
        let observed = ArtifactSeal::observe_all(artifacts).ok()?;
        let entries = self.locked_entries();
        let receipt = entries.get(key)?;
        (receipt.seals == observed).then(|| TransferableReceipt {
            seals: observed,
            value: receipt.value.clone(),
            stats: receipt.stats,
        })
    }

    /// Reuse a currently sealed verdict without creating a replacement on a
    /// miss. Producers use this when the fallback is a distinct bounded
    /// validation path that must not be promoted into the stronger receipt.
    pub fn reuse_sealed(&self, key: &K, artifacts: &[PathBuf]) -> Option<V> {
        let observed = ArtifactSeal::observe_all(artifacts).ok()?;
        let mut entries = self.locked_entries();
        let Some(receipt) = entries.get_mut(key) else {
            return None;
        };
        if receipt.seals != observed {
            entries.remove(key);
            return None;
        }
        receipt.stats.reuses = receipt.stats.reuses.saturating_add(1);
        Some(receipt.value.clone())
    }

    /// Transfer a deep-validation fact to paths just proven to be hard links
    /// of the validated source artifacts.
    ///
    /// The caller must invoke this only after its hard-link helper has verified
    /// native file identity. The transfer accepts the expected inode-change
    /// caused by link creation, but no byte-affecting metadata drift. Absent
    /// SQLite sidecars may pair with absent destination sidecars. Any mismatch
    /// returns `Ok(false)` and the destination must be deep-validated normally.
    pub fn install_hard_link_alias<E>(
        &self,
        source_key: &K,
        source_artifacts: &[PathBuf],
        destination_key: K,
        destination_artifacts: &[PathBuf],
        transferable: TransferableReceipt<V>,
        map_value: impl FnOnce(V) -> Result<V, E>,
    ) -> Result<bool, E> {
        let Ok(source_after) = ArtifactSeal::observe_all(source_artifacts) else {
            return Ok(false);
        };
        let Ok(destination_after) = ArtifactSeal::observe_all(destination_artifacts) else {
            return Ok(false);
        };
        if transferable.seals.len() != source_after.len()
            || source_after.len() != destination_after.len()
            || !transferable
                .seals
                .iter()
                .zip(&source_after)
                .all(|(before, after)| before.same_source_after_hard_link(after))
            || !source_after
                .iter()
                .zip(&destination_after)
                .all(|(source, destination)| source.same_hard_link_state(destination))
        {
            return Ok(false);
        }

        let source_value = transferable.value.clone();
        let value = map_value(transferable.value)?;
        if ArtifactSeal::observe_all(source_artifacts).ok().as_ref() != Some(&source_after)
            || ArtifactSeal::observe_all(destination_artifacts)
                .ok()
                .as_ref()
                != Some(&destination_after)
        {
            return Ok(false);
        }

        let mut entries = self.locked_entries();
        entries.remove(source_key);
        if self.capacity > 1 {
            if entries.len() >= self.capacity {
                entries.clear();
            }
            entries.insert(
                source_key.clone(),
                Receipt {
                    seals: source_after,
                    value: source_value,
                    stats: transferable.stats,
                },
            );
        }
        if entries.len() >= self.capacity && !entries.contains_key(&destination_key) {
            entries.clear();
        }
        let mut destination_stats = transferable.stats;
        destination_stats.reuses = destination_stats.reuses.saturating_add(1);
        entries.insert(
            destination_key,
            Receipt {
                seals: destination_after,
                value,
                stats: destination_stats,
            },
        );
        Ok(true)
    }

    /// Refresh the source key after one or more owned hard links changed only
    /// inode link metadata for artifacts covered by its receipt.
    pub fn refresh_after_hard_links(
        &self,
        key: K,
        artifacts: &[PathBuf],
        transferable: TransferableReceipt<V>,
    ) -> bool {
        let Ok(after) = ArtifactSeal::observe_all(artifacts) else {
            return false;
        };
        if transferable.seals.len() != after.len()
            || !transferable
                .seals
                .iter()
                .zip(&after)
                .all(|(before, after)| before.same_source_after_hard_link(after))
            || ArtifactSeal::observe_all(artifacts).ok().as_ref() != Some(&after)
        {
            return false;
        }
        let mut entries = self.locked_entries();
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            entries.clear();
        }
        let mut stats = transferable.stats;
        stats.reuses = stats.reuses.saturating_add(1);
        entries.insert(
            key,
            Receipt {
                seals: after,
                value: transferable.value,
                stats,
            },
        );
        true
    }

    /// Seal a value already validated by the artifact's owning producer.
    ///
    /// This is for staged producers that have constructed and checked the
    /// complete immutable value in memory before publication. Consumers must
    /// continue to use [`Self::validate_sealed`]; calling this without an
    /// owning construction proof would turn an assertion into evidence.
    pub fn seal_produced(&self, key: K, artifacts: &[PathBuf], value: V) -> bool {
        let Ok(observed) = ArtifactSeal::observe_all(artifacts) else {
            return false;
        };
        if ArtifactSeal::observe_all(artifacts).ok().as_ref() != Some(&observed) {
            return false;
        }
        let mut entries = self.locked_entries();
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            entries.clear();
        }
        entries.insert(
            key,
            Receipt {
                seals: observed,
                value,
                stats: ReceiptStats {
                    validations: 1,
                    reuses: 0,
                    invalidations: 0,
                },
            },
        );
        true
    }

    /// Accounting for `key`, or `None` when no receipt is held.
    pub fn stats(&self, key: &K) -> Option<ReceiptStats> {
        self.locked_entries().get(key).map(|entry| entry.stats)
    }

    /// Number of receipts currently held.
    pub fn len(&self) -> usize {
        self.locked_entries().len()
    }

    /// Whether no receipt is currently held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop every receipt.
    pub fn clear(&self) {
        self.locked_entries().clear();
    }

    fn locked_entries(&self) -> std::sync::MutexGuard<'_, HashMap<K, Receipt<V>>> {
        // A poisoned receipt cache is recoverable: the worst a torn map can do
        // is cost a re-validation, and refusing to validate would be worse.
        self.entries()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs::{File, FileTimes};
    use std::time::Duration;

    fn write(path: &Path, body: &str) {
        std::fs::write(path, body).expect("write artifact");
    }

    fn set_modified(path: &Path, nanos_since_epoch: u64) {
        let file = File::options()
            .write(true)
            .open(path)
            .expect("open artifact to set times");
        file.set_times(
            FileTimes::new()
                .set_modified(UNIX_EPOCH + Duration::from_nanos(nanos_since_epoch))
                .set_accessed(UNIX_EPOCH + Duration::from_nanos(nanos_since_epoch)),
        )
        .expect("set artifact times");
    }

    struct CountingValidator {
        runs: Cell<u64>,
    }

    impl CountingValidator {
        fn new() -> Self {
            Self { runs: Cell::new(0) }
        }

        fn ok(&self, value: &str) -> Result<String, String> {
            self.runs.set(self.runs.get() + 1);
            Ok(value.to_string())
        }

        fn err(&self) -> Result<String, String> {
            self.runs.set(self.runs.get() + 1);
            Err("validation failed".into())
        }
    }

    /// The declared fidelity has to match the platform the build is for.
    ///
    /// This is the statement the two detection tests below branch on. Pinning
    /// it separately keeps the pair honest: over-claiming
    /// [`SealFidelity::InodeChangeTracked`] on a platform that cannot deliver
    /// it would otherwise only surface as a silent receipt reuse of corrupted
    /// bytes.
    #[test]
    fn a_present_artifact_reports_the_fidelity_its_platform_actually_provides() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        write(&artifact, "generation-a");
        let fidelity = ArtifactSeal::observe(&artifact)
            .expect("seal the artifact")
            .fidelity();

        #[cfg(unix)]
        assert_eq!(
            fidelity,
            Some(SealFidelity::InodeChangeTracked),
            "Unix reports a device/inode pair and an inode-change instant"
        );
        #[cfg(not(unix))]
        assert_eq!(
            fidelity,
            Some(SealFidelity::TimestampsOnly),
            "Windows `std::fs` reports no device/inode pair and no inode-change instant; \
             claiming otherwise would let a corrupted generation answer from its receipt"
        );

        assert_eq!(
            ArtifactSeal::observe(&dir.path().join("absent.sqlite3"))
                .expect("seal an absent artifact")
                .fidelity(),
            None,
            "absence has no weaker form"
        );
    }

    #[test]
    fn an_unchanged_artifact_is_validated_once_and_reused() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        write(&artifact, "generation-a");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone()];

        let first = cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("verdict-a"))
            .expect("first validation");
        let second = cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("verdict-b"))
            .expect("second validation");

        assert_eq!(first, "verdict-a");
        assert_eq!(
            second, "verdict-a",
            "the sealed receipt must answer, not a fresh validation"
        );
        assert_eq!(validator.runs.get(), 1);
        assert_eq!(
            cache.stats(&artifact),
            Some(ReceiptStats {
                validations: 1,
                reuses: 1,
                invalidations: 0,
            })
        );
    }

    #[test]
    fn an_owned_hard_link_can_inherit_the_source_validation_without_reading_bytes() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let source = dir.path().join("source.sqlite3");
        let destination = dir.path().join("destination.sqlite3");
        let source_wal = dir.path().join("source.sqlite3-wal");
        let destination_wal = dir.path().join("destination.sqlite3-wal");
        write(&source, "immutable-generation");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let source_artifacts = vec![source.clone(), source_wal];
        let destination_artifacts = vec![destination.clone(), destination_wal];

        cache
            .validate_sealed(source.clone(), &source_artifacts, || {
                validator.ok("validated-contents")
            })
            .expect("validate source");
        let transferable = cache
            .transferable_receipt(&source, &source_artifacts)
            .expect("capture source receipt");
        std::fs::hard_link(&source, &destination).expect("create owned hard link");
        assert!(
            cache
                .install_hard_link_alias(
                    &source,
                    &source_artifacts,
                    destination.clone(),
                    &destination_artifacts,
                    transferable,
                    Ok::<_, String>,
                )
                .expect("install alias")
        );

        let inherited = cache
            .validate_sealed(destination.clone(), &destination_artifacts, || {
                validator.ok("unexpected-second-validation")
            })
            .expect("reuse destination receipt");
        assert_eq!(inherited, "validated-contents");
        assert_eq!(validator.runs.get(), 1);
        assert_eq!(
            cache.stats(&destination).map(|stats| stats.reuses),
            Some(2),
            "one reuse transfers the receipt and one answers the destination"
        );
    }

    #[test]
    fn a_transformed_hard_link_alias_preserves_the_source_receipt_value() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let source = dir.path().join("source.sqlite3");
        let destination = dir.path().join("destination.sqlite3");
        write(&source, "immutable-generation");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();

        cache
            .validate_sealed(source.clone(), std::slice::from_ref(&source), || {
                validator.ok("source-generation")
            })
            .expect("validate source");
        let transferable = cache
            .transferable_receipt(&source, std::slice::from_ref(&source))
            .expect("capture source receipt");
        std::fs::hard_link(&source, &destination).expect("create owned hard link");
        assert!(
            cache
                .install_hard_link_alias(
                    &source,
                    std::slice::from_ref(&source),
                    destination.clone(),
                    std::slice::from_ref(&destination),
                    transferable,
                    |value| Ok::<_, String>(format!("{value}-destination")),
                )
                .expect("install transformed alias")
        );

        let source_value = cache
            .validate_sealed(source.clone(), std::slice::from_ref(&source), || {
                validator.ok("unexpected-source-validation")
            })
            .expect("reuse source receipt");
        let destination_value = cache
            .validate_sealed(
                destination.clone(),
                std::slice::from_ref(&destination),
                || validator.ok("unexpected-destination-validation"),
            )
            .expect("reuse destination receipt");

        assert_eq!(source_value, "source-generation");
        assert_eq!(destination_value, "source-generation-destination");
        assert_eq!(validator.runs.get(), 1);
    }

    #[test]
    fn a_byte_copy_cannot_be_admitted_as_a_hard_link_alias() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let source = dir.path().join("source.sqlite3");
        let destination = dir.path().join("destination.sqlite3");
        write(&source, "immutable-generation");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        cache
            .validate_sealed(source.clone(), std::slice::from_ref(&source), || {
                Ok::<_, String>("validated-contents".to_string())
            })
            .expect("validate source");
        let transferable = cache
            .transferable_receipt(&source, std::slice::from_ref(&source))
            .expect("capture source receipt");
        std::fs::copy(&source, &destination).expect("copy bytes");

        assert!(
            !cache
                .install_hard_link_alias(
                    &source,
                    std::slice::from_ref(&source),
                    destination.clone(),
                    std::slice::from_ref(&destination),
                    transferable,
                    Ok::<_, String>,
                )
                .expect("refuse alias")
        );
        assert_eq!(cache.stats(&destination), None);
    }

    #[test]
    fn a_dependent_source_receipt_can_follow_owned_link_metadata_churn() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let source = dir.path().join("source.sqlite3");
        let destination = dir.path().join("destination.sqlite3");
        write(&source, "immutable-generation");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        cache
            .validate_sealed(source.clone(), std::slice::from_ref(&source), || {
                validator.ok("validated-contents")
            })
            .expect("validate source");
        let transferable = cache
            .transferable_receipt(&source, std::slice::from_ref(&source))
            .expect("capture source receipt");
        std::fs::hard_link(&source, &destination).expect("create owned hard link");
        assert!(cache.refresh_after_hard_links(
            source.clone(),
            std::slice::from_ref(&source),
            transferable,
        ));
        cache
            .validate_sealed(source.clone(), std::slice::from_ref(&source), || {
                validator.ok("unexpected-second-validation")
            })
            .expect("reuse refreshed source receipt");
        assert_eq!(validator.runs.get(), 1);
    }

    #[test]
    fn an_owning_producer_can_seal_its_checked_immutable_output() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("produced.sqlite3");
        write(&artifact, "producer-checked-generation");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        assert!(cache.seal_produced(
            artifact.clone(),
            std::slice::from_ref(&artifact),
            "producer-checked-value".to_string(),
        ));
        let observed = cache
            .validate_sealed(artifact.clone(), std::slice::from_ref(&artifact), || {
                validator.ok("unexpected-validation")
            })
            .expect("reuse producer receipt");
        assert_eq!(observed, "producer-checked-value");
        assert_eq!(validator.runs.get(), 0);

        write(&artifact, "producer-output-mutated");
        cache
            .validate_sealed(artifact.clone(), std::slice::from_ref(&artifact), || {
                validator.ok("revalidated")
            })
            .expect("mutation invalidates producer receipt");
        assert_eq!(validator.runs.get(), 1);
    }

    /// The seal's strength against a deliberate in-place rewrite, stated
    /// exactly as far as the platform can carry it.
    ///
    /// This test used to assert detection unconditionally. It runs only where
    /// `codestory-contracts` tests run — Linux and macOS — so the assertion was
    /// never contradicted, while the same code on Windows answers the rewritten
    /// artifact from the earlier receipt. Both outcomes are pinned here against
    /// the fidelity the observation itself reports, so neither platform's
    /// behaviour can drift and neither can be claimed for the other.
    #[test]
    fn an_in_place_rewrite_that_restores_the_modification_time_is_detected_only_at_full_fidelity() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        write(&artifact, "generation-a");
        let pinned_modified = 1_700_000_000_000_000_000;
        set_modified(&artifact, pinned_modified);
        let fidelity = ArtifactSeal::observe(&artifact)
            .expect("seal the artifact")
            .fidelity()
            .expect("a present artifact reports its fidelity");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone()];

        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("healthy"))
            .expect("seal the healthy generation");

        // Same path, same inode, same length, and the modification time is put
        // back exactly where it was: only the native inode-change instant
        // records that the bytes were rewritten.
        write(&artifact, "generation-X");
        set_modified(&artifact, pinned_modified);

        let after = cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("damaged"))
            .expect("re-validate after in-place corruption");

        match fidelity {
            SealFidelity::InodeChangeTracked => {
                assert_eq!(
                    after, "damaged",
                    "corrupted bytes must not be answered from the earlier receipt"
                );
                assert_eq!(validator.runs.get(), 2);
                assert_eq!(
                    cache.stats(&artifact),
                    Some(ReceiptStats {
                        validations: 2,
                        reuses: 0,
                        invalidations: 1,
                    })
                );
            }
            SealFidelity::TimestampsOnly => {
                // The stated limit, asserted rather than assumed: nothing in
                // the observation changed, so the receipt answers for bytes it
                // never read.
                assert_eq!(
                    after, "healthy",
                    "a timestamps-only seal cannot see a same-length rewrite"
                );
                assert_eq!(validator.runs.get(), 1);
                assert_eq!(
                    cache.stats(&artifact),
                    Some(ReceiptStats {
                        validations: 1,
                        reuses: 1,
                        invalidations: 0,
                    })
                );

                // What the platform does still guarantee: a rewrite that
                // changes the length breaks the seal.
                write(&artifact, "generation-X-longer");
                set_modified(&artifact, pinned_modified);
                cache
                    .validate_sealed(artifact.clone(), &artifacts, || validator.ok("resized"))
                    .expect("re-validate after a length change");
                assert_eq!(validator.runs.get(), 2);
            }
        }
    }

    /// Replacement detection, stated exactly as far as the platform can carry
    /// it.
    ///
    /// A timestamps-only observation carries no file identity, so whether a
    /// byte-identical replacement is noticed there depends on whether the new
    /// file happens to inherit the old creation instant — which on NTFS it
    /// sometimes does. The contract therefore claims nothing about that case
    /// on such a platform, and this test claims nothing either; it pins the
    /// residual guarantee instead.
    #[test]
    fn replacing_the_artifact_with_identical_bytes_is_detected_only_at_full_fidelity() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        let replacement = dir.path().join("shard.sqlite3.new");
        write(&artifact, "generation-a");
        let pinned_modified = 1_700_000_000_000_000_000;
        set_modified(&artifact, pinned_modified);
        let fidelity = ArtifactSeal::observe(&artifact)
            .expect("seal the artifact")
            .fidelity()
            .expect("a present artifact reports its fidelity");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone()];

        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("healthy"))
            .expect("seal the healthy generation");

        write(&replacement, "generation-a");
        set_modified(&replacement, pinned_modified);
        std::fs::rename(&replacement, &artifact).expect("replace artifact");

        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("rebuilt"))
            .expect("re-validate after replacement");

        match fidelity {
            SealFidelity::InodeChangeTracked => assert_eq!(
                validator.runs.get(),
                2,
                "a replaced file is a different artifact even when its bytes match"
            ),
            SealFidelity::TimestampsOnly => {
                // The residual guarantee: a replacement that does not restore
                // the modification time is still refused.
                let runs_after_identical_replacement = validator.runs.get();
                write(&replacement, "generation-a");
                set_modified(&replacement, pinned_modified + 1);
                std::fs::rename(&replacement, &artifact).expect("replace artifact again");
                cache
                    .validate_sealed(artifact.clone(), &artifacts, || validator.ok("rebuilt"))
                    .expect("re-validate after a retimed replacement");
                assert_eq!(
                    validator.runs.get(),
                    runs_after_identical_replacement + 1,
                    "a timestamps-only seal must still refuse a replacement it can see"
                );
            }
        }
    }

    #[test]
    fn a_sidecar_appearing_beside_the_artifact_breaks_the_seal() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        let sidecar = dir.path().join("shard.sqlite3-wal");
        write(&artifact, "generation-a");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone(), sidecar.clone()];

        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("healthy"))
            .expect("seal with the sidecar absent");
        write(&sidecar, "uncommitted pages");
        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("healthy"))
            .expect("re-validate with the sidecar present");

        assert_eq!(validator.runs.get(), 2);
        assert_eq!(
            cache.stats(&artifact).map(|stats| stats.invalidations),
            Some(1)
        );
    }

    #[test]
    fn a_failed_validation_is_never_sealed_and_evicts_the_earlier_receipt() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        write(&artifact, "generation-a");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone()];

        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("healthy"))
            .expect("seal the healthy generation");
        write(&artifact, "generation-a-damaged");

        let first_failure = cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.err())
            .expect_err("damaged generation fails validation");
        assert_eq!(first_failure, "validation failed");
        assert_eq!(cache.stats(&artifact), None, "failures leave no receipt");

        let second_failure = cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.err())
            .expect_err("a repeated failure re-runs validation");
        assert_eq!(second_failure, "validation failed");
        assert_eq!(validator.runs.get(), 3);
        assert_eq!(cache.stats(&artifact), None);
    }

    #[test]
    fn an_artifact_that_changes_during_validation_is_not_sealed() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        write(&artifact, "generation-a");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone()];

        cache
            .validate_sealed(artifact.clone(), &artifacts, || {
                let verdict = validator.ok("healthy");
                write(&artifact, "generation-a-rewritten-mid-validation");
                verdict
            })
            .expect("validation succeeds against the bytes it read");

        assert_eq!(
            cache.stats(&artifact),
            None,
            "a verdict about bytes that no longer exist must not be sealed"
        );
        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("healthy"))
            .expect("second validation");
        assert_eq!(validator.runs.get(), 2);
    }

    #[test]
    fn an_unsealable_artifact_validates_without_caching() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let artifact = dir.path().join("shard.sqlite3");
        std::fs::create_dir(&artifact).expect("create directory where a file belongs");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(4);
        let validator = CountingValidator::new();
        let artifacts = vec![artifact.clone()];

        assert_eq!(
            ArtifactSeal::observe(&artifact)
                .expect_err("a directory cannot be sealed")
                .code(),
            "artifact_seal_not_regular_file"
        );
        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("verdict"))
            .expect("first validation");
        cache
            .validate_sealed(artifact.clone(), &artifacts, || validator.ok("verdict"))
            .expect("second validation");

        assert_eq!(validator.runs.get(), 2);
        assert_eq!(cache.stats(&artifact), None);
    }

    #[test]
    fn the_receipt_cache_holds_at_most_its_capacity() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let cache: SealedReceiptCache<PathBuf, String> = SealedReceiptCache::new(2);
        let validator = CountingValidator::new();
        for index in 0..5 {
            let artifact = dir.path().join(format!("shard-{index}.sqlite3"));
            write(&artifact, "generation");
            cache
                .validate_sealed(artifact.clone(), std::slice::from_ref(&artifact), || {
                    validator.ok("verdict")
                })
                .expect("validate");
            assert!(cache.len() <= 2, "receipt cache exceeded its capacity");
        }
        assert_eq!(validator.runs.get(), 5);
    }
}
