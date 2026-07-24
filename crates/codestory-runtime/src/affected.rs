use super::{
    AffectedAnalysisBoundsDto, AffectedAnalysisCompletenessDto, AffectedAnalysisDto,
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedFollowUpDto, AffectedFollowUpInvocationDto, AffectedInputClassificationDto,
    AffectedMatchedFileDto, AffectedRouteDto, AffectedSymbolDto, AffectedTestFileDto,
    AffectedUncoveredInputDto, AffectedUnmatchedPathDto, ApiError, AppController, FileInfo,
    GraphNode, GraphNodeId, IndexFreshnessDto, IndexFreshnessStatusDto, IndexedFileRoleDto, NodeId,
    NodeKind, OperationPathIdentityResolver, PathIdentityUnavailable, Store, WorkspacePathIdentity,
    clamp_usize_to_u32, edge_certainty_label,
    index_freshness_observation_from_storage_with_identities, indexable_source_path_in_workspace,
    indexable_source_path_with_root, indexed_file_role, normalize_path_key, path_role_from_key,
    runtime_relative_path,
};
use crate::workspace_state::runtime_workspace_manifest;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "tests/affected.rs"]
pub(crate) mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AffectedConfidenceFloor {
    strength: u8,
    label: String,
}

impl AffectedConfidenceFloor {
    fn from_label(label: impl Into<String>) -> Self {
        let mut label = label.into();
        let strength = match label.as_str() {
            "direct" => 7,
            "certain" | "schema" => 6,
            "probable" | "file_convention" | "decorator" | "annotation" | "attribute" => 5,
            "graph" => 4,
            "heuristic" => 3,
            "uncertain" => 2,
            "bounded" => 1,
            _ => 1,
        };
        if !matches!(
            label.as_str(),
            "direct"
                | "certain"
                | "schema"
                | "probable"
                | "graph"
                | "heuristic"
                | "file_convention"
                | "decorator"
                | "annotation"
                | "attribute"
                | "uncertain"
                | "bounded"
        ) {
            label = "bounded".to_string();
        }
        Self { strength, label }
    }

    fn bounded() -> Self {
        Self::from_label("bounded")
    }

    fn weakest(&self, other: &Self) -> Self {
        match self.strength.cmp(&other.strength) {
            Ordering::Less => self.clone(),
            Ordering::Greater => other.clone(),
            Ordering::Equal if self.label <= other.label => self.clone(),
            Ordering::Equal => other.clone(),
        }
    }

    fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AffectedPathTieStep {
    edge_id: i64,
    source_node_id: GraphNodeId,
    target_node_id: GraphNodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AffectedGraphEvidence {
    distance: u32,
    reason: String,
    confidence_floor: AffectedConfidenceFloor,
    previous_identity_proxy: bool,
    path_tie_key: Vec<AffectedPathTieStep>,
}

impl AffectedGraphEvidence {
    fn seed(
        node_id: GraphNodeId,
        reason: impl Into<String>,
        confidence_floor: AffectedConfidenceFloor,
        previous_identity_proxy: bool,
    ) -> Self {
        Self {
            distance: 0,
            reason: reason.into(),
            confidence_floor: if previous_identity_proxy {
                AffectedConfidenceFloor::bounded()
            } else {
                confidence_floor
            },
            previous_identity_proxy,
            path_tie_key: vec![AffectedPathTieStep {
                edge_id: i64::MIN,
                source_node_id: node_id,
                target_node_id: node_id,
            }],
        }
    }
}

fn compare_affected_evidence(
    left: &AffectedGraphEvidence,
    right: &AffectedGraphEvidence,
) -> Ordering {
    left.distance
        .cmp(&right.distance)
        .then(
            left.previous_identity_proxy
                .cmp(&right.previous_identity_proxy),
        )
        .then_with(|| {
            right
                .confidence_floor
                .strength
                .cmp(&left.confidence_floor.strength)
        })
        .then(
            left.confidence_floor
                .label
                .cmp(&right.confidence_floor.label),
        )
        .then(left.path_tie_key.cmp(&right.path_tie_key))
        .then(left.reason.cmp(&right.reason))
}

fn affected_evidence_is_better(
    candidate: &AffectedGraphEvidence,
    current: &AffectedGraphEvidence,
) -> bool {
    compare_affected_evidence(candidate, current) == Ordering::Less
}

fn normalized_affected_input(
    input: &AffectedAnalysisInput,
) -> Result<(Vec<String>, Vec<AffectedChangeRecordDto>), ApiError> {
    let count = match input {
        AffectedAnalysisInput::Paths(paths) => paths.len(),
        AffectedAnalysisInput::ChangeRecords(records) => records.len(),
    };
    if !(1..=200).contains(&count) {
        return Err(ApiError::invalid_argument(
            "affected analysis requires between 1 and 200 path records",
        ));
    }

    match input {
        AffectedAnalysisInput::Paths(paths) => {
            let changed_paths = paths
                .iter()
                .map(|path| path.trim())
                .map(|path| {
                    if path.is_empty() {
                        Err(ApiError::invalid_argument(
                            "affected paths must be non-empty strings",
                        ))
                    } else {
                        Ok(path.to_string())
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let change_records = changed_paths
                .iter()
                .map(|path| AffectedChangeRecordDto {
                    path: path.clone(),
                    kind: AffectedChangeKindDto::Unknown,
                    status: "path".to_string(),
                    previous_path: None,
                })
                .collect();
            Ok((changed_paths, change_records))
        }
        AffectedAnalysisInput::ChangeRecords(records) => {
            let mut normalized_records = Vec::with_capacity(records.len());
            for record in records {
                let path = record.path.trim();
                if path.is_empty() {
                    return Err(ApiError::invalid_argument(
                        "affected change record paths must be non-empty strings",
                    ));
                }
                let previous_path = record
                    .previous_path
                    .as_deref()
                    .map(str::trim)
                    .map(|previous_path| {
                        if previous_path.is_empty() {
                            Err(ApiError::invalid_argument(
                                "affected previous paths must be non-empty strings",
                            ))
                        } else if !matches!(
                            &record.kind,
                            AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
                        ) {
                            Err(ApiError::invalid_argument(
                                "affected previous_path is valid only for renamed or copied records",
                            ))
                        } else {
                            Ok(previous_path.to_string())
                        }
                    })
                    .transpose()?;
                normalized_records.push(AffectedChangeRecordDto {
                    path: path.to_string(),
                    kind: record.kind.clone(),
                    status: record.status.clone(),
                    previous_path,
                });
            }
            Ok((
                normalized_records
                    .iter()
                    .map(|record| record.path.clone())
                    .collect(),
                normalized_records,
            ))
        }
    }
}

fn affected_change_path(root: &Path, raw: &str) -> PathBuf {
    let requested = Path::new(raw);
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    }
}

pub(super) type AffectedNativePathIdentityResolver = fn(&Path) -> io::Result<WorkspacePathIdentity>;

/// Native path observations and freshness membership for one affected call.
///
/// Keeping the resolver and identity-keyed refresh sets together prevents the
/// affected path from falling back to pairwise path comparisons or observing
/// one spelling twice after a concurrent replacement.
pub(super) struct AffectedOperationIdentityIndex<R = AffectedNativePathIdentityResolver> {
    resolver: OperationPathIdentityResolver<R>,
    pub(super) admitted_identities: HashSet<WorkspacePathIdentity>,
    pub(super) stale_identities: HashSet<WorkspacePathIdentity>,
    freshness_identity_gaps: BTreeMap<PathBuf, String>,
}

impl AffectedOperationIdentityIndex<AffectedNativePathIdentityResolver> {
    pub(super) fn native() -> Self {
        Self {
            resolver: OperationPathIdentityResolver::native(),
            admitted_identities: HashSet::new(),
            stale_identities: HashSet::new(),
            freshness_identity_gaps: BTreeMap::new(),
        }
    }
}

impl<R> AffectedOperationIdentityIndex<R>
where
    R: FnMut(&Path) -> io::Result<WorkspacePathIdentity>,
{
    #[cfg(test)]
    fn with_resolver(resolver: R) -> Self {
        Self {
            resolver: OperationPathIdentityResolver::with_resolver(resolver),
            admitted_identities: HashSet::new(),
            stale_identities: HashSet::new(),
            freshness_identity_gaps: BTreeMap::new(),
        }
    }

    fn resolve(&mut self, path: &Path) -> Result<WorkspacePathIdentity, PathIdentityUnavailable> {
        self.resolver.resolve(path)
    }

    pub(super) fn record_admitted(&mut self, path: &Path) {
        match self.resolve(path) {
            Ok(identity) => {
                self.admitted_identities.insert(identity);
            }
            Err(error) => self.record_freshness_gap(error),
        }
    }

    pub(super) fn record_stale(&mut self, path: &Path) {
        match self.resolve(path) {
            Ok(identity) => {
                self.stale_identities.insert(identity);
            }
            Err(error) => self.record_freshness_gap(error),
        }
    }

    fn record_freshness_gap(&mut self, error: PathIdentityUnavailable) {
        self.freshness_identity_gaps
            .entry(error.path.clone())
            .or_insert_with(|| error.to_string());
    }

    pub(super) fn freshness_identity_gap_count(&self) -> usize {
        self.freshness_identity_gaps.len()
    }

    pub(super) fn freshness_identity_gap_sample(&self) -> Option<String> {
        self.freshness_identity_gaps.values().next().cloned()
    }
}

struct AffectedIdentityMatches {
    matched_file_ids: HashSet<GraphNodeId>,
    matched_record_flags: Vec<bool>,
    matched_record_index_by_file_id: HashMap<GraphNodeId, usize>,
    graph_seeded_record_flags: Vec<bool>,
    previous_record_index_by_file_id: HashMap<GraphNodeId, usize>,
    current_identity_by_record: Vec<Option<WorkspacePathIdentity>>,
    previous_identity_by_record: Vec<Option<WorkspacePathIdentity>>,
    current_identity_error_by_record: Vec<Option<String>>,
    previous_identity_error_by_record: Vec<Option<String>>,
    indexed_identity_by_file_id: HashMap<GraphNodeId, WorkspacePathIdentity>,
    unavailable_indexed_identity_count: usize,
    unavailable_indexed_identity_sample: Option<String>,
    work: AffectedIdentityMatchWork,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AffectedIdentityMatchWork {
    record_visits: usize,
    indexed_file_visits: usize,
    current_identity_bucket_visits: usize,
    previous_identity_bucket_visits: usize,
    bucket_record_visits: usize,
    indexed_bucket_file_visits: usize,
}

#[derive(Debug, Default)]
struct AffectedOrderedIdentityBuckets {
    positions: HashMap<WorkspacePathIdentity, usize>,
    buckets: Vec<(WorkspacePathIdentity, Vec<usize>)>,
}

impl AffectedOrderedIdentityBuckets {
    fn push(&mut self, identity: WorkspacePathIdentity, record_index: usize) {
        if let Some(position) = self.positions.get(&identity).copied() {
            self.buckets[position].1.push(record_index);
            return;
        }
        let position = self.buckets.len();
        self.positions.insert(identity.clone(), position);
        self.buckets.push((identity, vec![record_index]));
    }
}

fn match_affected_file_identities<'a, I, R>(
    root: &Path,
    change_records: &[AffectedChangeRecordDto],
    indexed_files: I,
    path_identities: &mut AffectedOperationIdentityIndex<R>,
) -> AffectedIdentityMatches
where
    I: IntoIterator<Item = (GraphNodeId, &'a Path)>,
    R: FnMut(&Path) -> io::Result<WorkspacePathIdentity>,
{
    let mut work = AffectedIdentityMatchWork::default();
    let mut current_buckets = AffectedOrderedIdentityBuckets::default();
    let mut previous_buckets = AffectedOrderedIdentityBuckets::default();
    let mut current_identity_by_record = Vec::with_capacity(change_records.len());
    let mut previous_identity_by_record = Vec::with_capacity(change_records.len());
    let mut current_identity_error_by_record = Vec::with_capacity(change_records.len());
    let mut previous_identity_error_by_record = Vec::with_capacity(change_records.len());
    for (record_index, record) in change_records.iter().enumerate() {
        work.record_visits = work.record_visits.saturating_add(1);
        let current_path = affected_change_path(root, &record.path);
        match path_identities.resolve(&current_path) {
            Ok(identity) => {
                current_buckets.push(identity.clone(), record_index);
                current_identity_by_record.push(Some(identity));
                current_identity_error_by_record.push(None);
            }
            Err(error) => {
                current_identity_by_record.push(None);
                current_identity_error_by_record.push(Some(error.to_string()));
            }
        }

        if matches!(
            record.kind,
            AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
        ) && let Some(previous_path) = record.previous_path.as_deref()
        {
            let previous_path = affected_change_path(root, previous_path);
            match path_identities.resolve(&previous_path) {
                Ok(identity) => {
                    previous_buckets.push(identity.clone(), record_index);
                    previous_identity_by_record.push(Some(identity));
                    previous_identity_error_by_record.push(None);
                }
                Err(error) => {
                    previous_identity_by_record.push(None);
                    previous_identity_error_by_record.push(Some(error.to_string()));
                }
            }
        } else {
            previous_identity_by_record.push(None);
            previous_identity_error_by_record.push(None);
        }
    }

    let mut matched_file_ids = HashSet::new();
    let mut matched_record_flags = vec![false; change_records.len()];
    let mut matched_record_index_by_file_id = HashMap::<GraphNodeId, usize>::new();
    let mut graph_seeded_record_flags = vec![false; change_records.len()];
    let mut previous_record_index_by_file_id = HashMap::<GraphNodeId, usize>::new();
    let mut unavailable_indexed_identity_count = 0_usize;
    let mut unavailable_indexed_identity_sample = None::<(String, String)>;
    let mut indexed_identity_by_file_id = HashMap::new();
    let mut indexed_file_ids_by_identity =
        HashMap::<WorkspacePathIdentity, Vec<GraphNodeId>>::new();
    for (file_id, path) in indexed_files {
        work.indexed_file_visits = work.indexed_file_visits.saturating_add(1);
        let indexed_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        let identity = match path_identities.resolve(&indexed_path) {
            Ok(identity) => identity,
            Err(error) => {
                unavailable_indexed_identity_count += 1;
                let candidate = (
                    indexed_path.to_string_lossy().into_owned(),
                    error.to_string(),
                );
                if unavailable_indexed_identity_sample
                    .as_ref()
                    .is_none_or(|current| candidate < *current)
                {
                    unavailable_indexed_identity_sample = Some(candidate);
                }
                continue;
            }
        };
        indexed_identity_by_file_id.insert(file_id, identity.clone());
        indexed_file_ids_by_identity
            .entry(identity)
            .or_default()
            .push(file_id);
    }

    for (identity, record_indexes) in current_buckets.buckets {
        work.current_identity_bucket_visits = work.current_identity_bucket_visits.saturating_add(1);
        let Some(file_ids) = indexed_file_ids_by_identity.get(&identity) else {
            continue;
        };
        let mut first_record_index = None::<usize>;
        for record_index in record_indexes {
            work.bucket_record_visits = work.bucket_record_visits.saturating_add(1);
            matched_record_flags[record_index] = true;
            graph_seeded_record_flags[record_index] = true;
            first_record_index =
                Some(first_record_index.map_or(record_index, |current| current.min(record_index)));
        }
        let Some(first_record_index) = first_record_index else {
            continue;
        };
        for file_id in file_ids {
            work.indexed_bucket_file_visits = work.indexed_bucket_file_visits.saturating_add(1);
            matched_file_ids.insert(*file_id);
            matched_record_index_by_file_id
                .entry(*file_id)
                .and_modify(|current| *current = (*current).min(first_record_index))
                .or_insert(first_record_index);
        }
    }

    for (identity, record_indexes) in previous_buckets.buckets {
        work.previous_identity_bucket_visits =
            work.previous_identity_bucket_visits.saturating_add(1);
        let Some(file_ids) = indexed_file_ids_by_identity.get(&identity) else {
            continue;
        };
        let mut first_eligible_record_index = None::<usize>;
        for record_index in record_indexes {
            work.bucket_record_visits = work.bucket_record_visits.saturating_add(1);
            if matched_record_flags[record_index] {
                continue;
            }
            graph_seeded_record_flags[record_index] = true;
            first_eligible_record_index = Some(
                first_eligible_record_index
                    .map_or(record_index, |current| current.min(record_index)),
            );
        }
        let Some(first_eligible_record_index) = first_eligible_record_index else {
            continue;
        };
        for file_id in file_ids {
            work.indexed_bucket_file_visits = work.indexed_bucket_file_visits.saturating_add(1);
            if matched_file_ids.contains(file_id) {
                continue;
            }
            previous_record_index_by_file_id
                .entry(*file_id)
                .and_modify(|current| *current = (*current).min(first_eligible_record_index))
                .or_insert(first_eligible_record_index);
        }
    }

    AffectedIdentityMatches {
        matched_file_ids,
        matched_record_flags,
        matched_record_index_by_file_id,
        graph_seeded_record_flags,
        previous_record_index_by_file_id,
        current_identity_by_record,
        previous_identity_by_record,
        current_identity_error_by_record,
        previous_identity_error_by_record,
        indexed_identity_by_file_id,
        unavailable_indexed_identity_count,
        unavailable_indexed_identity_sample: unavailable_indexed_identity_sample
            .map(|(_, error)| error),
        work,
    }
}

#[derive(Debug, Clone)]
struct AffectedResolvedInput {
    current: PathBuf,
    previous: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(super) struct IndexFreshnessObservation {
    pub(super) freshness: IndexFreshnessDto,
    pub(super) inventory_complete: bool,
    pub(super) admitted_identities: HashSet<WorkspacePathIdentity>,
    pub(super) stale_identities: HashSet<WorkspacePathIdentity>,
    pub(super) identity_gap_count: usize,
    pub(super) identity_gap_sample: Option<String>,
}

impl IndexFreshnessObservation {
    pub(super) fn incomplete(freshness: IndexFreshnessDto) -> Self {
        Self {
            freshness,
            inventory_complete: false,
            admitted_identities: HashSet::new(),
            stale_identities: HashSet::new(),
            identity_gap_count: 0,
            identity_gap_sample: None,
        }
    }

    fn identity_is_admitted(&self, identity: &WorkspacePathIdentity) -> bool {
        self.admitted_identities.contains(identity)
    }

    fn identity_is_stale(&self, identity: &WorkspacePathIdentity) -> bool {
        self.stale_identities.contains(identity)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AffectedEvidenceGapCategory {
    count: usize,
    sample: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AffectedRelevantEvidenceGaps {
    current: AffectedEvidenceGapCategory,
    previous: AffectedEvidenceGapCategory,
    indexed: AffectedEvidenceGapCategory,
    freshness: AffectedEvidenceGapCategory,
}

struct AffectedRelevantEvidenceGapInput<'a> {
    workspace_root: Option<&'a Path>,
    resolved_inputs: &'a [AffectedResolvedInput],
    matched_record_flags: &'a [bool],
    current_identity_errors: &'a [Option<String>],
    previous_identity_errors: &'a [Option<String>],
    unavailable_indexed_identity_count: usize,
    unavailable_indexed_identity_sample: Option<&'a str>,
    freshness_evidence_affects_requested_claim: bool,
    freshness_identity_gap_count: usize,
    freshness_identity_gap_sample: Option<&'a str>,
}

fn affected_relevant_evidence_gaps(
    input: AffectedRelevantEvidenceGapInput<'_>,
) -> AffectedRelevantEvidenceGaps {
    let current_errors = input
        .current_identity_errors
        .iter()
        .enumerate()
        .filter_map(|(index, error)| {
            let error = error.as_deref()?;
            let resolved = input.resolved_inputs.get(index)?;
            (input
                .matched_record_flags
                .get(index)
                .copied()
                .unwrap_or(false)
                || indexable_source_path_with_root(input.workspace_root, &resolved.current))
            .then_some(error)
        })
        .collect::<Vec<_>>();
    let previous_errors = input
        .previous_identity_errors
        .iter()
        .enumerate()
        .filter_map(|(index, error)| {
            let error = error.as_deref()?;
            let resolved = input.resolved_inputs.get(index)?;
            (!input
                .matched_record_flags
                .get(index)
                .copied()
                .unwrap_or(false)
                && resolved.previous.as_deref().is_some_and(|path| {
                    indexable_source_path_with_root(input.workspace_root, path)
                }))
            .then_some(error)
        })
        .collect::<Vec<_>>();
    let indexed_identity_is_relevant =
        input
            .resolved_inputs
            .iter()
            .enumerate()
            .any(|(index, resolved)| {
                input
                    .matched_record_flags
                    .get(index)
                    .copied()
                    .unwrap_or(false)
                    || indexable_source_path_with_root(input.workspace_root, &resolved.current)
                    || resolved.previous.as_deref().is_some_and(|path| {
                        indexable_source_path_with_root(input.workspace_root, path)
                    })
            });

    AffectedRelevantEvidenceGaps {
        current: AffectedEvidenceGapCategory {
            count: current_errors.len(),
            sample: current_errors.first().map(|error| (*error).to_string()),
        },
        previous: AffectedEvidenceGapCategory {
            count: previous_errors.len(),
            sample: previous_errors.first().map(|error| (*error).to_string()),
        },
        indexed: AffectedEvidenceGapCategory {
            count: if indexed_identity_is_relevant {
                input.unavailable_indexed_identity_count
            } else {
                0
            },
            sample: if indexed_identity_is_relevant {
                input
                    .unavailable_indexed_identity_sample
                    .map(str::to_string)
            } else {
                None
            },
        },
        freshness: AffectedEvidenceGapCategory {
            count: if input.freshness_evidence_affects_requested_claim {
                input.freshness_identity_gap_count
            } else {
                0
            },
            sample: if input.freshness_evidence_affects_requested_claim {
                input.freshness_identity_gap_sample.map(str::to_string)
            } else {
                None
            },
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AffectedEvidenceGapComposition {
    gap_free: bool,
    unavailable_evidence_count: usize,
    blind_spots: Vec<String>,
}

impl AffectedEvidenceGapComposition {
    fn confidence(&self) -> &'static str {
        if self.gap_free { "complete" } else { "bounded" }
    }
}

fn compose_affected_evidence_gaps(
    base_unavailable_evidence_count: usize,
    gaps: &AffectedRelevantEvidenceGaps,
) -> AffectedEvidenceGapComposition {
    let unavailable_evidence_count = [
        base_unavailable_evidence_count,
        gaps.current.count,
        gaps.previous.count,
        gaps.indexed.count,
        gaps.freshness.count,
    ]
    .into_iter()
    .fold(0_usize, usize::saturating_add);
    let gap_free = gaps.current.count == 0
        && gaps.previous.count == 0
        && gaps.indexed.count == 0
        && gaps.freshness.count == 0;
    let mut blind_spots = Vec::new();
    if gaps.current.count > 0 {
        let detail = gaps
            .current
            .sample
            .as_deref()
            .unwrap_or("no identity error sample available");
        blind_spots.push(format!(
            "native current-path identity was unavailable for {} relevant changed records; completeness was downgraded ({detail})",
            gaps.current.count
        ));
    }
    if gaps.previous.count > 0 {
        let detail = gaps
            .previous
            .sample
            .as_deref()
            .unwrap_or("no identity error sample available");
        blind_spots.push(format!(
            "native previous-path identity was unavailable for {} unmatched rename/copy records; completeness was downgraded ({detail})",
            gaps.previous.count
        ));
    }
    if gaps.indexed.count > 0 {
        let detail = gaps
            .indexed
            .sample
            .as_deref()
            .unwrap_or("no identity error sample available");
        blind_spots.push(format!(
            "native path identity was unavailable for {} relevant indexed files; completeness was downgraded ({detail})",
            gaps.indexed.count
        ));
    }
    if gaps.freshness.count > 0 {
        let detail = gaps
            .freshness
            .sample
            .as_deref()
            .unwrap_or("no identity error sample available");
        blind_spots.push(format!(
            "native path identity was unavailable for {} refresh-plan paths needed by the requested claim ({detail})",
            gaps.freshness.count
        ));
    }

    AffectedEvidenceGapComposition {
        gap_free,
        unavailable_evidence_count,
        blind_spots,
    }
}

struct AffectedCompletenessInput {
    uncovered_input_count: usize,
    direct_impact_count: usize,
    propagated_impact_count: usize,
    candidate_test_count: usize,
    freshness_evidence_affects_requested_claim: bool,
    gap_composition: AffectedEvidenceGapComposition,
    truncation_reasons: Vec<String>,
}

fn compose_affected_completeness(
    input: AffectedCompletenessInput,
) -> AffectedAnalysisCompletenessDto {
    let truncated = !input.truncation_reasons.is_empty();
    let complete = input.uncovered_input_count == 0
        && !truncated
        && input.gap_composition.gap_free
        && !input.freshness_evidence_affects_requested_claim;
    AffectedAnalysisCompletenessDto {
        complete,
        confidence: if complete {
            "complete"
        } else if !input.gap_composition.gap_free {
            input.gap_composition.confidence()
        } else {
            "bounded"
        }
        .to_string(),
        direct_impact_count: clamp_usize_to_u32(input.direct_impact_count),
        propagated_impact_count: clamp_usize_to_u32(input.propagated_impact_count),
        candidate_test_count: clamp_usize_to_u32(input.candidate_test_count),
        uncovered_input_count: clamp_usize_to_u32(input.uncovered_input_count),
        unavailable_evidence_count: clamp_usize_to_u32(
            input.gap_composition.unavailable_evidence_count,
        ),
        truncated,
        truncation_reasons: input.truncation_reasons,
    }
}

#[derive(Debug)]
enum AffectedPathMetadataObservation {
    RegularFile,
    NonRegular,
    Missing,
    Unavailable {
        kind: io::ErrorKind,
        message: String,
    },
}

struct AffectedUnmatchedPathObservation<'a> {
    workspace_root: Option<&'a Path>,
    metadata: AffectedPathMetadataObservation,
}

fn affected_path_metadata(path: &Path) -> AffectedPathMetadataObservation {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            AffectedPathMetadataObservation::RegularFile
        }
        Ok(_) => AffectedPathMetadataObservation::NonRegular,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            AffectedPathMetadataObservation::Missing
        }
        Err(error) => AffectedPathMetadataObservation::Unavailable {
            kind: error.kind(),
            message: error.to_string(),
        },
    }
}

fn classify_unmatched_affected_input(
    workspace_root: Option<&Path>,
    record: &AffectedChangeRecordDto,
    resolved: &AffectedResolvedInput,
    freshness: &IndexFreshnessObservation,
    current_identity: Option<&WorkspacePathIdentity>,
    current_identity_error: Option<&str>,
    previous_identity_error: Option<&str>,
) -> (AffectedInputClassificationDto, String, Vec<String>) {
    classify_unmatched_affected_input_with_metadata(
        record,
        resolved,
        freshness,
        current_identity,
        current_identity_error,
        previous_identity_error,
        AffectedUnmatchedPathObservation {
            workspace_root,
            metadata: affected_path_metadata(&resolved.current),
        },
    )
}

fn classify_unmatched_affected_input_with_metadata(
    record: &AffectedChangeRecordDto,
    resolved: &AffectedResolvedInput,
    freshness: &IndexFreshnessObservation,
    current_identity: Option<&WorkspacePathIdentity>,
    current_identity_error: Option<&str>,
    previous_identity_error: Option<&str>,
    observation: AffectedUnmatchedPathObservation<'_>,
) -> (AffectedInputClassificationDto, String, Vec<String>) {
    let mut evidence = Vec::new();
    let path = &resolved.current;
    let regular_file = match observation.metadata {
        AffectedPathMetadataObservation::RegularFile => {
            evidence.push(format!(
                "resolved existing regular project file: {}",
                path.display()
            ));
            true
        }
        AffectedPathMetadataObservation::NonRegular => {
            evidence.push(format!(
                "resolved existing non-regular project path: {}",
                path.display()
            ));
            return (
                AffectedInputClassificationDto::Malformed,
                "resolved project path is not a regular file".to_string(),
                evidence,
            );
        }
        AffectedPathMetadataObservation::Missing => {
            evidence.push(format!("resolved missing project path: {}", path.display()));
            false
        }
        AffectedPathMetadataObservation::Unavailable { kind, message } => {
            evidence.push(format!(
                "metadata read failed for resolved project path {}: kind={kind:?} error={message}",
                path.display()
            ));
            return (
                AffectedInputClassificationDto::UnavailableEvidence,
                "resolved project path metadata was unavailable, so existence and regular-file status could not be established"
                    .to_string(),
                evidence,
            );
        }
    };

    if regular_file {
        if !indexable_source_path_with_root(observation.workspace_root, path) {
            return (
                AffectedInputClassificationDto::ValidUncovered,
                "regular file exists inside the project but is outside current graph/index coverage"
                    .to_string(),
                evidence,
            );
        }
        if let Some(error) = current_identity_error {
            evidence.push(format!(
                "native current-path identity was unavailable: {error}"
            ));
            return (
                AffectedInputClassificationDto::UnavailableEvidence,
                "path exists and is indexable, but native identity evidence was unavailable"
                    .to_string(),
                evidence,
            );
        }
        let Some(current_identity) = current_identity else {
            evidence.push("native current-path identity was not observed".to_string());
            return (
                AffectedInputClassificationDto::UnavailableEvidence,
                "path exists and is indexable, but native identity evidence was unavailable"
                    .to_string(),
                evidence,
            );
        };
        if freshness.identity_is_stale(current_identity) {
            evidence
                .push("complete freshness evidence identifies this path as changed or new".into());
            return (
                AffectedInputClassificationDto::StaleIndex,
                "path is indexable and complete freshness evidence shows the publication is stale for it"
                    .to_string(),
                evidence,
            );
        }
        if freshness.inventory_complete && !freshness.identity_is_admitted(current_identity) {
            evidence.push(
                "complete workspace inventory excludes this path from the admitted index set"
                    .into(),
            );
            return (
                AffectedInputClassificationDto::ValidUncovered,
                "regular file exists inside the project but is excluded from current graph/index coverage"
                    .to_string(),
                evidence,
            );
        }
        if !freshness.inventory_complete {
            evidence.push(
                "bounded freshness or refresh-plan identity evidence was unavailable for this indexable path"
                    .into(),
            );
            return (
                AffectedInputClassificationDto::UnavailableEvidence,
                "path exists and is indexable, but bounded freshness evidence cannot explain its graph absence"
                    .to_string(),
                evidence,
            );
        }
        evidence.push(
            "path is indexable, but no indexed row or exact stale/error evidence was available"
                .into(),
        );
        return (
            AffectedInputClassificationDto::UnavailableEvidence,
            "path exists and is indexable, but current evidence cannot explain its absence from the graph"
                .to_string(),
            evidence,
        );
    }

    if let Some(error) = current_identity_error {
        evidence.push(format!(
            "native current-path identity was unavailable: {error}"
        ));
        return (
            AffectedInputClassificationDto::UnavailableEvidence,
            "missing-path identity was unavailable, so indexed membership could not be established"
                .to_string(),
            evidence,
        );
    }

    match record.kind {
        AffectedChangeKindDto::Deleted => (
            AffectedInputClassificationDto::ExpectedDeleted,
            "deleted path is absent from both the workspace and indexed file inventory".to_string(),
            evidence,
        ),
        AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied => {
            if let Some(error) = previous_identity_error {
                evidence.push(format!(
                    "native previous-path identity was unavailable: {error}"
                ));
                return (
                    AffectedInputClassificationDto::UnavailableEvidence,
                    "previous rename/copy identity was unavailable, so bounded proxy coverage could not be established"
                        .to_string(),
                    evidence,
                );
            }
            if let Some(previous) = resolved.previous.as_deref() {
                match affected_path_metadata(previous) {
                    AffectedPathMetadataObservation::RegularFile => {
                        evidence.push("previous path exists as a regular file".to_string());
                    }
                    AffectedPathMetadataObservation::NonRegular => {
                        evidence.push("previous path exists but is not a regular file".to_string());
                    }
                    AffectedPathMetadataObservation::Missing => {
                        evidence.push("previous path is missing".to_string());
                    }
                    AffectedPathMetadataObservation::Unavailable { kind, message } => {
                        evidence.push(format!(
                            "metadata read failed for resolved previous project path {}: kind={kind:?} error={message}",
                            previous.display()
                        ));
                        return (
                            AffectedInputClassificationDto::UnavailableEvidence,
                            "previous rename/copy path metadata was unavailable, so its file state could not be established"
                                .to_string(),
                            evidence,
                        );
                    }
                }
            }
            (
                AffectedInputClassificationDto::RenameUnresolved,
                "neither the current nor previous rename/copy path matched indexed file identity"
                    .to_string(),
                evidence,
            )
        }
        _ => (
            AffectedInputClassificationDto::Missing,
            "path does not exist inside the project and did not match indexed file identity"
                .to_string(),
            evidence,
        ),
    }
}

fn classify_matched_affected_input(
    file: &AffectedMatchedFileDto,
    stale: bool,
    freshness_available: bool,
) -> Option<(AffectedInputClassificationDto, String, Vec<String>)> {
    if file.error_count > 0 {
        return Some((
            AffectedInputClassificationDto::Malformed,
            "indexed path has exact recorded parse/index errors".to_string(),
            vec![format!("indexed file error_count={}", file.error_count)],
        ));
    }
    if stale {
        return Some((
            AffectedInputClassificationDto::StaleIndex,
            "matched path has exact complete-inventory evidence of stale publication".to_string(),
            vec!["complete freshness evidence identifies this path as stale".to_string()],
        ));
    }
    if !freshness_available {
        return Some((
            AffectedInputClassificationDto::UnavailableEvidence,
            "matched file identity is known, but bounded freshness evidence was unavailable"
                .to_string(),
            vec![
                "bounded freshness or refresh-plan identity evidence was unavailable for this matched path"
                    .to_string(),
            ],
        ));
    }
    if file.indexed && file.complete {
        return None;
    }
    Some((
        AffectedInputClassificationDto::UnavailableEvidence,
        "matched file is incomplete or not indexed without an exact recorded error".to_string(),
        vec![format!(
            "indexed={} complete={} error_count={}",
            file.indexed, file.complete, file.error_count
        )],
    ))
}

fn affected_files_follow_up_invocation(project: &str, path: &str) -> AffectedFollowUpInvocationDto {
    AffectedFollowUpInvocationDto {
        program: "codestory-cli".to_string(),
        args: vec![
            "files".to_string(),
            "--project".to_string(),
            project.to_string(),
            "--path".to_string(),
            path.to_string(),
            "--format".to_string(),
            "markdown".to_string(),
        ],
    }
}

fn affected_follow_ups(
    project: &str,
    uncovered_inputs: &[AffectedUncoveredInputDto],
    freshness_diagnostic: Option<&str>,
) -> Vec<AffectedFollowUpDto> {
    let mut inputs = uncovered_inputs.iter().collect::<Vec<_>>();
    inputs.sort_by(|left, right| {
        affected_classification_rank(&left.classification)
            .cmp(&affected_classification_rank(&right.classification))
            .then(left.path.cmp(&right.path))
            .then(left.reason.cmp(&right.reason))
    });
    let mut follow_ups = BTreeMap::<(u8, String, String), AffectedFollowUpDto>::new();
    for input in inputs {
        let (priority, dedupe_path, follow_up) = match input.classification {
            AffectedInputClassificationDto::ValidUncovered => (
                5,
                input.path.clone(),
                AffectedFollowUpDto {
                    action: "inspect_graph_boundary".to_string(),
                    reason: format!(
                        "{} is a valid project file outside current graph coverage; source inspection is the relevant next step",
                        input.path
                    ),
                    confidence: "direct".to_string(),
                    invocation: None,
                },
            ),
            AffectedInputClassificationDto::StaleIndex => (
                0,
                String::new(),
                AffectedFollowUpDto {
                    action: "refresh_stale_index".to_string(),
                    reason:
                        "complete requested-path freshness evidence shows the publication is stale"
                            .to_string(),
                    confidence: "direct".to_string(),
                    invocation: Some(AffectedFollowUpInvocationDto {
                        program: "codestory-cli".to_string(),
                        args: vec![
                            "index".to_string(),
                            "--project".to_string(),
                            project.to_string(),
                            "--refresh".to_string(),
                            "incremental".to_string(),
                        ],
                    }),
                },
            ),
            AffectedInputClassificationDto::Missing
            | AffectedInputClassificationDto::RenameUnresolved
            | AffectedInputClassificationDto::ExpectedDeleted => (
                1,
                input.path.clone(),
                AffectedFollowUpDto {
                    action: "locate_input_path".to_string(),
                    reason: format!(
                        "confirm the exact project-relative spelling and file state for {}",
                        input.path
                    ),
                    confidence: "direct".to_string(),
                    invocation: Some(affected_files_follow_up_invocation(project, &input.path)),
                },
            ),
            AffectedInputClassificationDto::Malformed => {
                let recorded_error = input
                    .evidence
                    .iter()
                    .any(|evidence| evidence.starts_with("indexed file error_count="));
                (
                    2,
                    input.path.clone(),
                    AffectedFollowUpDto {
                        action: if recorded_error {
                            "inspect_recorded_index_error"
                        } else {
                            "inspect_malformed_input"
                        }
                        .to_string(),
                        reason: if recorded_error {
                            format!(
                                "inspect the exact recorded parse/index error for {} before retrying impact analysis",
                                input.path
                            )
                        } else {
                            format!(
                                "confirm {} is a regular project file before retrying impact analysis",
                                input.path
                            )
                        },
                        confidence: "direct".to_string(),
                        invocation: Some(affected_files_follow_up_invocation(project, &input.path)),
                    },
                )
            }
            AffectedInputClassificationDto::UnavailableEvidence => (
                3,
                input.path.clone(),
                AffectedFollowUpDto {
                    action: "inspect_input_evidence".to_string(),
                    reason: format!(
                        "inspect the focused inventory row for {}; current evidence cannot classify its graph absence more strongly",
                        input.path
                    ),
                    confidence: "bounded".to_string(),
                    invocation: Some(affected_files_follow_up_invocation(project, &input.path)),
                },
            ),
        };
        follow_ups
            .entry((priority, follow_up.action.clone(), dedupe_path))
            .or_insert(follow_up);
    }
    if let Some(reason) = freshness_diagnostic {
        let follow_up = AffectedFollowUpDto {
            action: "establish_freshness_evidence".to_string(),
            reason: reason.to_string(),
            confidence: "bounded".to_string(),
            invocation: Some(AffectedFollowUpInvocationDto {
                program: "codestory-cli".to_string(),
                args: vec![
                    "doctor".to_string(),
                    "--project".to_string(),
                    project.to_string(),
                    "--format".to_string(),
                    "markdown".to_string(),
                ],
            }),
        };
        follow_ups.insert((4, follow_up.action.clone(), String::new()), follow_up);
    }
    follow_ups.into_values().collect()
}

fn affected_classification_rank(classification: &AffectedInputClassificationDto) -> u8 {
    match classification {
        AffectedInputClassificationDto::StaleIndex => 0,
        AffectedInputClassificationDto::Missing
        | AffectedInputClassificationDto::ExpectedDeleted
        | AffectedInputClassificationDto::RenameUnresolved => 1,
        AffectedInputClassificationDto::Malformed => 2,
        AffectedInputClassificationDto::UnavailableEvidence => 3,
        AffectedInputClassificationDto::ValidUncovered => 4,
    }
}

fn affected_edge_kind_label(kind: codestory_contracts::graph::EdgeKind) -> &'static str {
    match kind {
        codestory_contracts::graph::EdgeKind::MEMBER => "member",
        codestory_contracts::graph::EdgeKind::TYPE_USAGE => "type_usage",
        codestory_contracts::graph::EdgeKind::USAGE => "usage",
        codestory_contracts::graph::EdgeKind::CALL => "call",
        codestory_contracts::graph::EdgeKind::INHERITANCE => "inheritance",
        codestory_contracts::graph::EdgeKind::OVERRIDE => "override",
        codestory_contracts::graph::EdgeKind::TYPE_ARGUMENT => "type_argument",
        codestory_contracts::graph::EdgeKind::TEMPLATE_SPECIALIZATION => "template_specialization",
        codestory_contracts::graph::EdgeKind::INCLUDE => "include",
        codestory_contracts::graph::EdgeKind::IMPORT => "import",
        codestory_contracts::graph::EdgeKind::MACRO_USAGE => "macro_usage",
        codestory_contracts::graph::EdgeKind::ANNOTATION_USAGE => "annotation_usage",
        codestory_contracts::graph::EdgeKind::UNKNOWN => "unknown",
    }
}

fn affected_edge_confidence(edge: &codestory_contracts::graph::Edge) -> AffectedConfidenceFloor {
    AffectedConfidenceFloor::from_label(
        edge_certainty_label(edge.kind, edge.certainty, edge.confidence)
            .unwrap_or_else(|| "graph".to_string()),
    )
}

fn affected_dependent_evidence(
    distance: u32,
    edge: &codestory_contracts::graph::Edge,
    target_label: String,
    parent: &AffectedGraphEvidence,
) -> AffectedGraphEvidence {
    let mut reason = format!(
        "dependent reaches changed code via {} edge to {}",
        affected_edge_kind_label(edge.kind),
        target_label
    );
    let mut confidence_floor = parent
        .confidence_floor
        .weakest(&affected_edge_confidence(edge));
    if parent.previous_identity_proxy {
        reason = format!("bounded previous-identity proxy; {reason}");
        confidence_floor = AffectedConfidenceFloor::bounded();
    }
    let mut path_tie_key = parent.path_tie_key.clone();
    path_tie_key.push(AffectedPathTieStep {
        edge_id: edge.id.0,
        source_node_id: edge.effective_source(),
        target_node_id: edge.effective_target(),
    });
    AffectedGraphEvidence {
        distance,
        reason,
        confidence_floor,
        previous_identity_proxy: parent.previous_identity_proxy,
        path_tie_key,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AffectedReverseWalk {
    distances: BTreeMap<GraphNodeId, u32>,
    evidence: BTreeMap<GraphNodeId, AffectedGraphEvidence>,
    visited_edge_count: usize,
}

fn compare_affected_reverse_edges(
    left: &codestory_contracts::graph::Edge,
    right: &codestory_contracts::graph::Edge,
) -> Ordering {
    left.effective_source()
        .cmp(&right.effective_source())
        .then(left.id.cmp(&right.id))
        .then(left.effective_target().cmp(&right.effective_target()))
        .then((left.kind as i32).cmp(&(right.kind as i32)))
        .then(left.line.cmp(&right.line))
        .then(left.source.cmp(&right.source))
        .then(left.target.cmp(&right.target))
        .then(left.resolved_source.cmp(&right.resolved_source))
        .then(left.resolved_target.cmp(&right.resolved_target))
        .then(left.callsite_identity.cmp(&right.callsite_identity))
}

fn affected_reverse_walk(
    depth: u32,
    edges: &[codestory_contracts::graph::Edge],
    seed_evidence: BTreeMap<GraphNodeId, AffectedGraphEvidence>,
    labels: &HashMap<GraphNodeId, String>,
) -> AffectedReverseWalk {
    let mut reverse_dependents = BTreeMap::<GraphNodeId, Vec<usize>>::new();
    for (edge_index, edge) in edges.iter().enumerate() {
        reverse_dependents
            .entry(edge.effective_target())
            .or_default()
            .push(edge_index);
    }
    for edge_indexes in reverse_dependents.values_mut() {
        edge_indexes
            .sort_by(|left, right| compare_affected_reverse_edges(&edges[*left], &edges[*right]));
    }

    let mut distances = BTreeMap::<GraphNodeId, u32>::new();
    let mut evidence = BTreeMap::<GraphNodeId, AffectedGraphEvidence>::new();
    for (seed, seed_evidence) in seed_evidence {
        distances.insert(seed, 0);
        evidence.insert(seed, seed_evidence);
    }
    let mut frontier = distances.keys().copied().collect::<Vec<_>>();
    let mut visited_edge_count = 0_usize;
    for distance in 0..depth {
        let next_distance = distance + 1;
        let mut next_candidates = BTreeMap::<GraphNodeId, AffectedGraphEvidence>::new();
        for node_id in frontier {
            let Some(parent_evidence) = evidence.get(&node_id) else {
                continue;
            };
            for edge_index in reverse_dependents.get(&node_id).into_iter().flatten() {
                visited_edge_count = visited_edge_count.saturating_add(1);
                let edge = &edges[*edge_index];
                let dependent = edge.effective_source();
                if distances.contains_key(&dependent) {
                    continue;
                }
                let target_label = labels
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| node_id.0.to_string());
                let candidate =
                    affected_dependent_evidence(next_distance, edge, target_label, parent_evidence);
                next_candidates
                    .entry(dependent)
                    .and_modify(|current| {
                        if affected_evidence_is_better(&candidate, current) {
                            current.clone_from(&candidate);
                        }
                    })
                    .or_insert(candidate);
            }
        }
        frontier = Vec::with_capacity(next_candidates.len());
        for (node_id, candidate) in next_candidates {
            distances.insert(node_id, next_distance);
            evidence.insert(node_id, candidate);
            frontier.push(node_id);
        }
        if frontier.is_empty() {
            break;
        }
    }

    AffectedReverseWalk {
        distances,
        evidence,
        visited_edge_count,
    }
}

fn affected_route_confidence(
    graph_evidence: &AffectedGraphEvidence,
    route_confidence: Option<&str>,
) -> String {
    let route_floor = AffectedConfidenceFloor::from_label(route_confidence.unwrap_or("graph"));
    graph_evidence
        .confidence_floor
        .weakest(&route_floor)
        .label()
        .to_string()
}

struct AffectedGraphIndex {
    labels: HashMap<GraphNodeId, String>,
    file_path_by_id: HashMap<GraphNodeId, String>,
    nodes_by_id: HashMap<GraphNodeId, GraphNode>,
    node_ids_by_file: HashMap<GraphNodeId, Vec<GraphNodeId>>,
}

fn affected_graph_index(
    controller: &AppController,
    root: &Path,
    files: &[FileInfo],
    nodes: &[GraphNode],
) -> AffectedGraphIndex {
    let mut labels = controller.cached_labels(nodes.iter().map(|node| node.id));
    for node in nodes {
        labels.entry(node.id).or_insert_with(|| {
            node.qualified_name
                .clone()
                .unwrap_or_else(|| node.serialized_name.clone())
        });
    }
    let file_path_by_id = files
        .iter()
        .map(|file| {
            (
                codestory_contracts::graph::NodeId(file.id),
                runtime_relative_path(root, &file.path),
            )
        })
        .collect();
    let nodes_by_id = nodes.iter().map(|node| (node.id, node.clone())).collect();
    let mut node_ids_by_file = HashMap::<GraphNodeId, Vec<GraphNodeId>>::new();
    for node in nodes {
        if let Some(file_id) = node.file_node_id {
            node_ids_by_file.entry(file_id).or_default().push(node.id);
        }
    }
    for node_ids in node_ids_by_file.values_mut() {
        node_ids.sort_unstable();
    }
    AffectedGraphIndex {
        labels,
        file_path_by_id,
        nodes_by_id,
        node_ids_by_file,
    }
}

fn affected_graph_seeds(
    graph_seed_file_ids: &BTreeSet<GraphNodeId>,
    previous_identity_seed_evidence: &BTreeMap<GraphNodeId, AffectedGraphEvidence>,
    node_ids_by_file: &HashMap<GraphNodeId, Vec<GraphNodeId>>,
) -> BTreeMap<GraphNodeId, AffectedGraphEvidence> {
    let mut seeds = BTreeMap::<GraphNodeId, AffectedGraphEvidence>::new();
    for file_id in graph_seed_file_ids {
        let file_evidence = previous_identity_seed_evidence.get(file_id).cloned();
        let file_candidate = file_evidence.clone().unwrap_or_else(|| {
            AffectedGraphEvidence::seed(
                *file_id,
                "changed file matched current input path",
                AffectedConfidenceFloor::from_label("direct"),
                false,
            )
        });
        seeds
            .entry(*file_id)
            .and_modify(|current| {
                if affected_evidence_is_better(&file_candidate, current) {
                    current.clone_from(&file_candidate);
                }
            })
            .or_insert(file_candidate);
        for node_id in node_ids_by_file.get(file_id).into_iter().flatten() {
            let node_candidate = file_evidence
                .as_ref()
                .map(|evidence| {
                    AffectedGraphEvidence::seed(
                        *node_id,
                        evidence.reason.clone(),
                        evidence.confidence_floor.clone(),
                        evidence.previous_identity_proxy,
                    )
                })
                .unwrap_or_else(|| {
                    AffectedGraphEvidence::seed(
                        *node_id,
                        "symbol declared in file matched by current input path",
                        AffectedConfidenceFloor::from_label("direct"),
                        false,
                    )
                });
            seeds
                .entry(*node_id)
                .and_modify(|current| {
                    if affected_evidence_is_better(&node_candidate, current) {
                        current.clone_from(&node_candidate);
                    }
                })
                .or_insert(node_candidate);
        }
    }
    seeds
}

struct AffectedSymbolImpacts {
    symbols: Vec<AffectedSymbolDto>,
    total: usize,
    truncated: bool,
    direct_count: usize,
    propagated_count: usize,
}

fn affected_symbol_impacts(
    distances: &BTreeMap<GraphNodeId, u32>,
    evidence: &BTreeMap<GraphNodeId, AffectedGraphEvidence>,
    graph: &AffectedGraphIndex,
    filter: Option<&str>,
) -> AffectedSymbolImpacts {
    let mut symbols = distances
        .iter()
        .filter_map(|(node_id, distance)| {
            let node = graph.nodes_by_id.get(node_id)?;
            if node.kind == codestory_contracts::graph::NodeKind::FILE {
                return None;
            }
            let file_path = node
                .file_node_id
                .and_then(|file_id| graph.file_path_by_id.get(&file_id).cloned());
            if filter.is_some_and(|needle| {
                !graph
                    .labels
                    .get(node_id)
                    .is_some_and(|label| normalize_path_key(label).contains(needle))
                    && !file_path
                        .as_deref()
                        .is_some_and(|path| normalize_path_key(path).contains(needle))
            }) {
                return None;
            }
            let graph_evidence = evidence.get(node_id).cloned().unwrap_or_else(|| {
                let mut fallback = AffectedGraphEvidence::seed(
                    *node_id,
                    "reached by dependent graph walk",
                    AffectedConfidenceFloor::from_label("graph"),
                    false,
                );
                fallback.distance = *distance;
                fallback
            });
            Some(AffectedSymbolDto {
                node_id: NodeId::from(*node_id),
                display_name: graph
                    .labels
                    .get(node_id)
                    .cloned()
                    .unwrap_or_else(|| node.serialized_name.clone()),
                kind: NodeKind::from(node.kind),
                file_path,
                line: node.start_line,
                distance: *distance,
                graph_depth: graph_evidence.distance,
                reason: graph_evidence.reason,
                confidence: graph_evidence.confidence_floor.label().to_string(),
            })
        })
        .collect::<Vec<_>>();
    let direct_count = symbols.iter().filter(|symbol| symbol.distance == 0).count();
    let propagated_count = symbols.iter().filter(|symbol| symbol.distance > 0).count();
    symbols.sort_by(|left, right| {
        left.distance
            .cmp(&right.distance)
            .then(left.file_path.cmp(&right.file_path))
            .then(left.display_name.cmp(&right.display_name))
            .then(left.node_id.0.cmp(&right.node_id.0))
    });
    let total = symbols.len();
    let truncated = total > 200;
    symbols.truncate(200);
    AffectedSymbolImpacts {
        symbols,
        total,
        truncated,
        direct_count,
        propagated_count,
    }
}

fn affected_test_impacts(symbols: &[AffectedSymbolDto]) -> Vec<AffectedTestFileDto> {
    let mut by_file = BTreeMap::<String, (u32, u32, String)>::new();
    for symbol in symbols {
        if let Some(path) = symbol.file_path.as_deref()
            && path_role_from_key(&normalize_path_key(path)) == IndexedFileRoleDto::Test
        {
            let entry = by_file.entry(path.to_string()).or_insert((
                0,
                symbol.graph_depth,
                symbol.confidence.clone(),
            ));
            entry.0 += 1;
            if symbol.graph_depth < entry.1 {
                entry.1 = symbol.graph_depth;
                entry.2.clone_from(&symbol.confidence);
            }
        }
    }
    by_file
        .into_iter()
        .map(
            |(path, (impacted_symbol_count, distance, confidence))| AffectedTestFileDto {
                path,
                reason: "focused test hint: test-like path reached by affected graph walk"
                    .to_string(),
                confidence,
                distance,
                graph_depth: distance,
                impacted_symbol_count,
            },
        )
        .collect()
}

struct AffectedRouteImpacts {
    routes: Vec<AffectedRouteDto>,
    total: usize,
    truncated: bool,
}

impl AppController {
    fn affected_route_impacts(
        &self,
        storage: &Store,
        distances: &BTreeMap<GraphNodeId, u32>,
        evidence: &BTreeMap<GraphNodeId, AffectedGraphEvidence>,
        graph: &AffectedGraphIndex,
        filter: Option<&str>,
    ) -> AffectedRouteImpacts {
        let mut routes = distances
            .iter()
            .filter_map(|(node_id, distance)| {
                let node = graph.nodes_by_id.get(node_id)?;
                let file_path = node
                    .file_node_id
                    .and_then(|file_id| graph.file_path_by_id.get(&file_id).cloned());
                if filter.is_some_and(|needle| {
                    !graph
                        .labels
                        .get(node_id)
                        .is_some_and(|label| normalize_path_key(label).contains(needle))
                        && !file_path
                            .as_deref()
                            .is_some_and(|path| normalize_path_key(path).contains(needle))
                }) {
                    return None;
                }
                let display_name = graph
                    .labels
                    .get(node_id)
                    .cloned()
                    .unwrap_or_else(|| node.serialized_name.clone());
                let route = self.route_endpoint_metadata(
                    storage,
                    node,
                    file_path.as_deref(),
                    &display_name,
                )?;
                let graph_evidence = evidence.get(node_id).cloned().unwrap_or_else(|| {
                    let mut fallback = AffectedGraphEvidence::seed(
                        *node_id,
                        "route endpoint reached by dependent graph walk",
                        AffectedConfidenceFloor::from_label("graph"),
                        false,
                    );
                    fallback.distance = *distance;
                    fallback
                });
                let confidence =
                    affected_route_confidence(&graph_evidence, route.confidence.as_deref());
                Some(AffectedRouteDto {
                    node_id: NodeId::from(*node_id),
                    display_name,
                    file_path,
                    line: node.start_line,
                    distance: *distance,
                    graph_depth: graph_evidence.distance,
                    reason: graph_evidence.reason,
                    confidence,
                    route,
                })
            })
            .collect::<Vec<_>>();
        routes.sort_by(|left, right| {
            left.distance
                .cmp(&right.distance)
                .then(left.file_path.cmp(&right.file_path))
                .then(left.display_name.cmp(&right.display_name))
                .then(left.node_id.0.cmp(&right.node_id.0))
        });
        let total = routes.len();
        let truncated = total > 100;
        routes.truncate(100);
        AffectedRouteImpacts {
            routes,
            total,
            truncated,
        }
    }

    pub fn affected_analysis(
        &self,
        req: AffectedAnalysisRequest,
    ) -> Result<AffectedAnalysisDto, ApiError> {
        self.ensure_consistent_read_state("Affected analysis")?;
        let root = self.require_project_root()?;
        let depth = req.depth.unwrap_or(2).clamp(1, 8);
        let filter = req.filter.as_deref().map(normalize_path_key);
        let (changed_paths, change_records) = normalized_affected_input(&req.input)?;
        let resolved_inputs = change_records
            .iter()
            .map(|record| {
                let current = self.resolve_project_file_path(&record.path, true)?;
                let previous = record
                    .previous_path
                    .as_deref()
                    .map(|path| self.resolve_project_file_path(path, true))
                    .transpose()?;
                Ok(AffectedResolvedInput { current, previous })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let storage = self.open_storage_read_only()?;
        let files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load indexed files: {e}")))?;
        let nodes = storage
            .get_nodes()
            .map_err(|e| ApiError::internal(format!("Failed to load graph nodes: {e}")))?;
        let edges = storage
            .get_edges()
            .map_err(|e| ApiError::internal(format!("Failed to load graph edges: {e}")))?;
        let errors = storage
            .get_errors(None)
            .map_err(|e| ApiError::internal(format!("Failed to load index errors: {e}")))?;
        let mut errors_by_file = HashMap::<i64, u32>::new();
        for error in errors {
            if let Some(file_id) = error.file_id {
                *errors_by_file.entry(file_id.0).or_default() += 1;
            }
        }

        let storage_path = self.require_storage_path()?;
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|error| ApiError::internal(format!("Failed to open project: {error}")))?;
        let mut path_identities = AffectedOperationIdentityIndex::native();
        let freshness_observation = index_freshness_observation_from_storage_with_identities(
            &root,
            &workspace,
            &storage,
            &self.source_index_policy,
            &mut path_identities,
        );
        let freshness = &freshness_observation.freshness;
        let identity_matches = match_affected_file_identities(
            &root,
            &change_records,
            files.iter().map(|file| {
                (
                    codestory_contracts::graph::NodeId(file.id),
                    file.path.as_path(),
                )
            }),
            &mut path_identities,
        );
        let AffectedIdentityMatches {
            matched_file_ids: current_matched_file_ids,
            matched_record_flags: current_matched_record_flags,
            matched_record_index_by_file_id,
            graph_seeded_record_flags,
            previous_record_index_by_file_id,
            current_identity_by_record,
            previous_identity_by_record: _previous_identity_by_record,
            current_identity_error_by_record,
            previous_identity_error_by_record,
            indexed_identity_by_file_id,
            unavailable_indexed_identity_count,
            unavailable_indexed_identity_sample,
            work: identity_match_work,
        } = identity_matches;
        tracing::debug!(
            record_visits = identity_match_work.record_visits,
            indexed_file_visits = identity_match_work.indexed_file_visits,
            current_identity_bucket_visits = identity_match_work.current_identity_bucket_visits,
            previous_identity_bucket_visits = identity_match_work.previous_identity_bucket_visits,
            bucket_record_visits = identity_match_work.bucket_record_visits,
            indexed_bucket_file_visits = identity_match_work.indexed_bucket_file_visits,
            "affected identity matching work"
        );
        let previous_identity_seed_evidence = previous_record_index_by_file_id
            .into_iter()
            .map(|(file_id, record_index)| {
                let record = &change_records[record_index];
                (
                    file_id,
                    AffectedGraphEvidence::seed(
                        file_id,
                        format!(
                            "previous indexed identity {} proxy-seeded graph evidence for current {} path {}",
                            record.previous_path.as_deref().unwrap_or_default(),
                            match &record.kind {
                                AffectedChangeKindDto::Renamed => "renamed",
                                AffectedChangeKindDto::Copied => "copied",
                                _ => unreachable!("previous identity seeds are rename/copy only"),
                            },
                            record.path,
                        ),
                        AffectedConfidenceFloor::bounded(),
                        true,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let graph_seed_file_ids = current_matched_file_ids
            .iter()
            .copied()
            .chain(previous_identity_seed_evidence.keys().copied())
            .collect::<BTreeSet<_>>();
        let mut matched_files = files
            .iter()
            .filter(|file| {
                current_matched_file_ids.contains(&codestory_contracts::graph::NodeId(file.id))
            })
            .map(|file| {
                let file_id = codestory_contracts::graph::NodeId(file.id);
                let record = matched_record_index_by_file_id
                    .get(&file_id)
                    .map(|record_index| &change_records[*record_index]);
                (
                    file_id,
                    AffectedMatchedFileDto {
                        path: runtime_relative_path(&root, &file.path),
                        role: indexed_file_role(&file.path),
                        indexed: file.indexed,
                        complete: file.complete,
                        change_kind: record.map(|record| record.kind.clone()),
                        change_status: record.map(|record| record.status.clone()),
                        previous_path: record.and_then(|record| record.previous_path.clone()),
                        error_count: errors_by_file.get(&file.id).copied().unwrap_or_default(),
                    },
                )
            })
            .collect::<Vec<_>>();
        matched_files
            .sort_by(|left, right| left.1.path.cmp(&right.1.path).then(left.0.cmp(&right.0)));
        let mut unmatched_freshness_unavailable = false;
        let unmatched_paths = change_records
            .iter()
            .enumerate()
            .filter(|(index, _)| !current_matched_record_flags[*index])
            .map(|(index, record)| {
                let (classification, mut reason, mut evidence) = classify_unmatched_affected_input(
                    Some(&root),
                    record,
                    &resolved_inputs[index],
                    &freshness_observation,
                    current_identity_by_record[index].as_ref(),
                    current_identity_error_by_record[index].as_deref(),
                    previous_identity_error_by_record[index].as_deref(),
                );
                unmatched_freshness_unavailable |= matches!(
                    affected_path_metadata(&resolved_inputs[index].current),
                    AffectedPathMetadataObservation::RegularFile
                ) && indexable_source_path_in_workspace(&root, &resolved_inputs[index].current)
                    && current_identity_error_by_record[index].is_none()
                    && current_identity_by_record[index]
                        .as_ref()
                        .is_some_and(|identity| !freshness_observation.identity_is_stale(identity))
                    && !freshness_observation.inventory_complete;
                if graph_seeded_record_flags[index] {
                    evidence.push(format!(
                        "previous indexed identity {} supplied bounded proxy graph evidence",
                        record.previous_path.as_deref().unwrap_or_default()
                    ));
                    if classification == AffectedInputClassificationDto::RenameUnresolved {
                        reason = "current rename/copy path remains unresolved; the previous indexed identity supplies bounded graph evidence but does not establish current-path coverage"
                            .to_string();
                    }
                }
                AffectedUnmatchedPathDto {
                    path: record.path.clone(),
                    classification,
                    reason,
                    evidence,
                    change_kind: Some(record.kind.clone()),
                    change_status: Some(record.status.clone()),
                    previous_path: record.previous_path.clone(),
                }
            })
            .collect::<Vec<_>>();
        let mut uncovered_inputs = unmatched_paths
            .iter()
            .map(|input| AffectedUncoveredInputDto {
                path: input.path.clone(),
                classification: input.classification.clone(),
                reason: input.reason.clone(),
                evidence: input.evidence.clone(),
            })
            .collect::<Vec<_>>();
        let mut matched_freshness_unavailable = false;
        for (file_id, file) in &matched_files {
            let stale = indexed_identity_by_file_id
                .get(file_id)
                .is_some_and(|identity| freshness_observation.identity_is_stale(identity));
            matched_freshness_unavailable |=
                !stale && file.error_count == 0 && !freshness_observation.inventory_complete;
            let Some((classification, reason, evidence)) = classify_matched_affected_input(
                file,
                stale,
                freshness_observation.inventory_complete
                    && freshness.status != IndexFreshnessStatusDto::NotChecked,
            ) else {
                continue;
            };
            uncovered_inputs.push(AffectedUncoveredInputDto {
                path: file.path.clone(),
                classification,
                reason,
                evidence,
            });
        }
        let matched_files = matched_files
            .into_iter()
            .map(|(_, file)| file)
            .collect::<Vec<_>>();

        let graph = affected_graph_index(self, &root, &files, &nodes);

        let seed_evidence = affected_graph_seeds(
            &graph_seed_file_ids,
            &previous_identity_seed_evidence,
            &graph.node_ids_by_file,
        );
        let AffectedReverseWalk {
            distances,
            evidence,
            visited_edge_count,
        } = affected_reverse_walk(depth, &edges, seed_evidence, &graph.labels);

        let symbol_impacts =
            affected_symbol_impacts(&distances, &evidence, &graph, filter.as_deref());
        let impacted_symbols = symbol_impacts.symbols;
        let direct_impact_count = symbol_impacts.direct_count;
        let propagated_impact_count = symbol_impacts.propagated_count;
        let impacted_symbol_total = symbol_impacts.total;
        let impacted_symbols_truncated = symbol_impacts.truncated;

        let route_impacts =
            self.affected_route_impacts(&storage, &distances, &evidence, &graph, filter.as_deref());
        let impacted_routes = route_impacts.routes;
        let impacted_route_total = route_impacts.total;
        let impacted_routes_truncated = route_impacts.truncated;
        let impacted_tests = affected_test_impacts(&impacted_symbols);

        let mut notes = Vec::new();
        let mut blind_spots = Vec::new();
        let freshness_evidence_affects_requested_claim =
            unmatched_freshness_unavailable || matched_freshness_unavailable;
        let relevant_evidence_gaps =
            affected_relevant_evidence_gaps(AffectedRelevantEvidenceGapInput {
                workspace_root: Some(&root),
                resolved_inputs: &resolved_inputs,
                matched_record_flags: &current_matched_record_flags,
                current_identity_errors: &current_identity_error_by_record,
                previous_identity_errors: &previous_identity_error_by_record,
                unavailable_indexed_identity_count,
                unavailable_indexed_identity_sample: unavailable_indexed_identity_sample.as_deref(),
                freshness_evidence_affects_requested_claim,
                freshness_identity_gap_count: freshness_observation.identity_gap_count,
                freshness_identity_gap_sample: freshness_observation.identity_gap_sample.as_deref(),
            });
        let unavailable_input_count = uncovered_inputs
            .iter()
            .filter(|input| {
                input.classification == AffectedInputClassificationDto::UnavailableEvidence
            })
            .count();
        let gap_composition =
            compose_affected_evidence_gaps(unavailable_input_count, &relevant_evidence_gaps);
        if current_matched_file_ids.is_empty() {
            let note = "no current input paths matched indexed file identity; inspect the typed uncovered-input evidence"
                .to_string();
            notes.push(note.clone());
            if graph_seed_file_ids.is_empty() {
                blind_spots.push(note);
            }
        } else {
            notes.push(format!(
                "matched {} indexed files by current path; dependency walk expanded files into contained symbols",
                current_matched_file_ids.len()
            ));
        }
        let graph_seeded_input_count = graph_seeded_record_flags
            .iter()
            .filter(|seeded| **seeded)
            .count();
        let graph_unseeded_input_count = change_records
            .len()
            .saturating_sub(graph_seeded_input_count);
        if graph_unseeded_input_count > 0 {
            blind_spots.push(format!(
                "{graph_unseeded_input_count} inputs had no indexed current or previous identity and were excluded from graph traversal"
            ));
        }
        let previous_only_seed_count = graph_seeded_record_flags
            .iter()
            .zip(&current_matched_record_flags)
            .filter(|(seeded, matched)| **seeded && !**matched)
            .count();
        if previous_only_seed_count > 0 {
            notes.push(format!(
                "{previous_only_seed_count} current input paths were classified separately while their previous indexed identities seeded graph traversal"
            ));
        }
        blind_spots.extend(gap_composition.blind_spots.iter().cloned());
        if matched_files
            .iter()
            .any(|file| !file.complete || file.error_count > 0)
        {
            blind_spots.push(
                "one or more matched files are incomplete or have recorded index errors"
                    .to_string(),
            );
        }
        if impacted_routes.is_empty() {
            blind_spots.push(
                "no route/endpoint evidence found for matched files or dependents".to_string(),
            );
        }
        if impacted_tests.is_empty() {
            notes.push("no impacted test-like files found in the indexed graph".to_string());
        }
        let mut truncation_reasons = Vec::new();
        if impacted_symbols_truncated {
            truncation_reasons.push(format!(
                "impacted_symbols retained 200 of {impacted_symbol_total} results"
            ));
        }
        if impacted_routes_truncated {
            truncation_reasons.push(format!(
                "impacted_routes retained 100 of {impacted_route_total} results"
            ));
        }
        let truncated = !truncation_reasons.is_empty();
        if truncated {
            blind_spots.extend(truncation_reasons.iter().cloned());
        }

        let project = root.to_string_lossy().to_string();
        if freshness.status == IndexFreshnessStatusDto::Stale
            && !uncovered_inputs
                .iter()
                .any(|input| input.classification == AffectedInputClassificationDto::StaleIndex)
        {
            blind_spots.push(
                "the workspace has unrelated stale index state, but no requested input was classified stale"
                    .to_string(),
            );
        }
        let freshness_diagnostic = freshness_evidence_affects_requested_claim.then(|| {
            freshness
                .reason
                .as_deref()
                .map(|reason| {
                    format!(
                        "inspect observational freshness because requested-path evidence was unavailable: {reason}"
                    )
                })
                .unwrap_or_else(|| {
                    "inspect observational freshness because requested-path refresh-plan identity evidence was unavailable"
                        .to_string()
                })
        });
        if freshness_diagnostic.is_some() {
            blind_spots.push(
                "bounded index freshness evidence was unavailable for a requested indexable path; no complete no-impact claim is possible"
                    .to_string(),
            );
        }
        let follow_ups =
            affected_follow_ups(&project, &uncovered_inputs, freshness_diagnostic.as_deref());
        let completeness = compose_affected_completeness(AffectedCompletenessInput {
            uncovered_input_count: uncovered_inputs.len(),
            direct_impact_count,
            propagated_impact_count,
            candidate_test_count: impacted_tests.len(),
            freshness_evidence_affects_requested_claim,
            gap_composition,
            truncation_reasons,
        });
        let bounds = AffectedAnalysisBoundsDto {
            requested_depth: depth,
            maximum_depth: 8,
            visited_node_count: clamp_usize_to_u32(distances.len()),
            visited_edge_count: clamp_usize_to_u32(visited_edge_count),
            impacted_symbol_limit: 200,
            impacted_route_limit: 100,
        };

        Ok(AffectedAnalysisDto {
            project_root: project,
            changed_paths,
            change_records,
            matched_files,
            unmatched_paths,
            uncovered_inputs,
            matched_file_count: current_matched_file_ids.len().min(u32::MAX as usize) as u32,
            depth,
            impacted_symbols,
            impacted_routes,
            impacted_tests,
            bounds,
            completeness,
            blind_spots,
            follow_ups,
            notes,
        })
    }
}
