use std::collections::HashMap;
use std::path::PathBuf;

/// Versioned policy that permits a verified bounded source to remain outside scheduling.
pub const OVERSIZED_SOURCE_POLICY_VERSION: &str = "bounded-source-exclusion-v2";
/// Admission headroom for parser-backed sources. **Not a cost bound.**
///
/// A source at or under this size is admitted for scheduling. It does not
/// promise that the work which follows is cheap: indexing cost tracks a file's
/// *shape*, not its size — at a fixed 500 KB, varying only statements per
/// function moves `index_file` between 1.2 s and 134 s, and the quadratic
/// loops behind that (#1820) are unfixed. Parsing itself is linear and small.
///
/// Raise this to widen what may be indexed. Do not read it as a budget, and do
/// not soften this comment when #1820 lands — the headroom is only defensible
/// once those loops are gone.
pub const DEFAULT_SOURCE_FILE_BYTE_CAP: u64 = 2_000_000;
/// Byte bound for structural formats, deliberately below the parser headroom.
///
/// A structural source's projected value is already bounded by
/// [`DEFAULT_STRUCTURAL_UNIT_CAP`]; past this size a larger file yields no more
/// units, only more bytes to walk. Keeping it at 1 MiB is why raising the
/// parser headroom cannot turn a 1.5 MB JSON into an indexing failure.
pub const DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP: u64 = 1024 * 1024;
/// Structural-unit bound shared by the structural collector and exclusion publication.
pub const DEFAULT_STRUCTURAL_UNIT_CAP: u64 = 2_048;
/// Immutable source-index policy shared by planning, parsing, publication, and reads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SourceIndexPolicy {
    pub policy_version: String,
    pub byte_cap: u64,
    #[serde(default = "default_structural_source_byte_cap")]
    pub structural_byte_cap: u64,
    pub structural_unit_cap: u64,
}

fn default_structural_source_byte_cap() -> u64 {
    DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP
}

impl SourceIndexPolicy {
    pub fn oversized(byte_cap: u64) -> Self {
        Self {
            policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
            byte_cap: byte_cap.max(1),
            structural_byte_cap: DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
            structural_unit_cap: DEFAULT_STRUCTURAL_UNIT_CAP,
        }
    }

    /// The cap that governs admission for `path`, and the cap a resulting
    /// exclusion record must name.
    ///
    /// `path` must be workspace-relative: the structural classifier consults
    /// path shape, so repository ancestors must not influence admission.
    ///
    /// The clamp is load-bearing. An operator lowering the headroom below the
    /// structural bound must not end up with structural files admitted above
    /// it — a row claiming a cap above the published headroom is rejected at
    /// publication.
    pub fn effective_byte_cap(&self, path: &str) -> u64 {
        if crate::language_support::is_structural_source_path(path) {
            self.structural_byte_cap.min(self.byte_cap)
        } else {
            self.byte_cap
        }
    }

    /// No path is excluded below this, so callers can skip classifying a file
    /// smaller than it.
    pub fn minimum_byte_cap(&self) -> u64 {
        self.byte_cap.min(self.structural_byte_cap)
    }
}

impl Default for SourceIndexPolicy {
    fn default() -> Self {
        Self::oversized(DEFAULT_SOURCE_FILE_BYTE_CAP)
    }
}

/// Content-verified oversized source classified before parser scheduling.
///
/// Project, workspace, and core-publication identity are deliberately absent here. The
/// runtime binds those identities only when the complete candidate set is published.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OversizedSourceExclusionCandidate {
    pub normalized_path: String,
    pub content_hash: String,
    pub observed_size: u64,
    /// Zero for a byte-bound exclusion; otherwise the collector-observed unit count.
    pub observed_unit_count: u64,
    pub policy_version: String,
    pub byte_cap: u64,
    pub structural_unit_cap: u64,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RefreshMode {
    Incremental,
    FullRefresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFileState {
    pub id: i64,
    pub path: PathBuf,
    pub modification_time: i64,
    pub content_hash: Option<String>,
    pub indexed: bool,
    pub complete: bool,
    pub retry_required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshInputs {
    pub stored_files: Vec<StoredFileState>,
    /// Complete verified exclusions from the currently published core.
    ///
    /// Structurally over-bound sources intentionally have no parser-backed
    /// file row, so refresh planning must carry their exact content identity
    /// separately to avoid rediscovering unchanged exclusions as new files.
    pub policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    pub inventory: WorkspaceInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFileRecord {
    pub file_id: i64,
    pub modification_time: i64,
    pub content_hash: Option<String>,
    pub indexed: bool,
    pub complete: bool,
    pub retry_required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceInventory {
    files: HashMap<PathBuf, IndexedFileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshPlan {
    pub mode: RefreshMode,
    pub files_to_index: Vec<PathBuf>,
    pub files_to_remove: Vec<i64>,
    pub existing_file_ids: HashMap<PathBuf, i64>,
}

impl RefreshInputs {
    pub fn inventory_map(&self) -> HashMap<PathBuf, StoredFileState> {
        if !self.stored_files.is_empty() {
            return self
                .stored_files
                .iter()
                .cloned()
                .map(|file| (file.path.clone(), file))
                .collect();
        }

        self.inventory
            .files
            .clone()
            .into_iter()
            .map(|(path, record)| {
                (
                    path.clone(),
                    StoredFileState {
                        id: record.file_id,
                        path,
                        modification_time: record.modification_time,
                        content_hash: record.content_hash,
                        indexed: record.indexed,
                        complete: record.complete,
                        retry_required: record.retry_required,
                    },
                )
            })
            .collect()
    }
}

impl WorkspaceInventory {
    pub fn from_records<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (PathBuf, IndexedFileRecord)>,
    {
        Self {
            files: iter.into_iter().collect(),
        }
    }
}

pub type BuildMode = RefreshMode;
pub type RefreshExecutionPlan = RefreshPlan;
pub type RefreshInfo = RefreshPlan;
pub type StoredFileRecord = StoredFileState;

#[cfg(test)]
mod tests {
    use super::*;

    /// T1: the premise of not bumping `OVERSIZED_SOURCE_POLICY_VERSION`.
    ///
    /// `byte_cap` moved from "the policy's cap" to "the cap that refused this
    /// row", which is a real change of meaning and would normally demand a
    /// version bump. It does not here, because no *valid* stored core can
    /// exist in a configuration where the two readings disagree: above
    /// 1 MiB, structural files were never excluded at all — they were
    /// scheduled and failed the whole refresh, so no clean core was produced.
    ///
    /// This test pins that argument. If it ever fails, the version bump
    /// becomes mandatory, because a stale core could then validate clean while
    /// meaning something different.
    #[test]
    fn effective_byte_cap_matches_the_legacy_classifier_wherever_a_stored_core_can_exist() {
        let paths = [
            "src/main.rs",
            "app/index.ts",
            "data/config.json",
            "ci/build.yaml",
            "Cargo.toml",
            ".github/workflows/ci.yml",
            "docker-compose.yml",
            "docs/guide.md",
        ];
        for byte_cap in [
            1,
            2,
            1_000_000,
            DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP - 1,
            DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
        ] {
            let policy = SourceIndexPolicy {
                byte_cap,
                ..SourceIndexPolicy::default()
            };
            for path in paths {
                assert_eq!(
                    policy.effective_byte_cap(path),
                    byte_cap,
                    "at or below the structural bound every path must resolve to \
                     the headroom itself, or an already-published core changes \
                     meaning under the new classifier ({path} at {byte_cap})"
                );
            }
        }
    }

    /// The clamp is what makes T1 hold: without it a structural path at
    /// `byte_cap = 2` would answer 1 MiB and a row would claim more headroom
    /// than the manifest published.
    #[test]
    fn a_structural_path_never_resolves_above_the_headroom() {
        let policy = SourceIndexPolicy {
            byte_cap: 2,
            ..SourceIndexPolicy::default()
        };
        assert_eq!(policy.effective_byte_cap("data/config.json"), 2);
        assert_eq!(policy.minimum_byte_cap(), 2);
    }

    /// Above the structural bound the two caps genuinely diverge — that is the
    /// whole point of the change, and it is what stops a 1.5 MB JSON from
    /// failing an index once the headroom is 2 MB.
    #[test]
    fn a_structural_path_keeps_its_own_bound_under_the_raised_headroom() {
        let policy = SourceIndexPolicy::default();
        assert_eq!(policy.byte_cap, DEFAULT_SOURCE_FILE_BYTE_CAP);
        assert_eq!(
            policy.effective_byte_cap("data/config.json"),
            DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP
        );
        assert_eq!(
            policy.effective_byte_cap("src/main.rs"),
            DEFAULT_SOURCE_FILE_BYTE_CAP
        );
        assert_eq!(
            policy.minimum_byte_cap(),
            DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP
        );
        const { assert!(DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP < DEFAULT_SOURCE_FILE_BYTE_CAP) };
    }

    /// A policy deserialized before `structural_byte_cap` existed must come
    /// back with the structural bound, not zero — a zero cap is rejected by
    /// planning and would refuse every file.
    #[test]
    fn a_policy_serialized_without_the_structural_cap_defaults_it() {
        let legacy = r#"{"policy_version":"bounded-source-exclusion-v2",
                         "byte_cap":1000000,"structural_unit_cap":2048}"#;
        let policy: SourceIndexPolicy = serde_json::from_str(legacy).expect("legacy policy");
        assert_eq!(
            policy.structural_byte_cap,
            DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP
        );
    }
}
