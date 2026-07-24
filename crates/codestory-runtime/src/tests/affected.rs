use super::{
    AffectedCompletenessInput, AffectedConfidenceFloor, AffectedEvidenceGapCategory,
    AffectedGraphEvidence, AffectedOperationIdentityIndex, AffectedPathMetadataObservation,
    AffectedPathTieStep, AffectedRelevantEvidenceGapInput, AffectedRelevantEvidenceGaps,
    AffectedResolvedInput, AffectedUnmatchedPathObservation, IndexFreshnessObservation,
    affected_follow_ups, affected_relevant_evidence_gaps, affected_reverse_walk,
    affected_route_confidence, classify_matched_affected_input, classify_unmatched_affected_input,
    classify_unmatched_affected_input_with_metadata, compose_affected_completeness,
    compose_affected_evidence_gaps, match_affected_file_identities, normalized_affected_input,
};
use crate::tests::assert_no_staged_publication_artifacts;
use crate::{
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedInputClassificationDto, AffectedMatchedFileDto, AffectedUncoveredInputDto, ApiError,
    AppController, EventBus, FileCoverageReason, FileInfo, HashMap, IndexFreshnessChangeKindDto,
    IndexFreshnessDto, IndexFreshnessSampleDto, IndexFreshnessStatusDto, IndexMode,
    IndexedFileRoleDto, OpenProjectRequest, PublicationTestAction, PublicationTestBoundary,
    RefreshExecutionPlan, RefreshMode, RouteHandlerCandidate, SourceIndexPolicy, Storage, Uuid,
    V2WorkspaceIndexer, WorkspaceManifest, arm_after_index_freshness_fence_test_hook,
    arm_publication_test_fault, compare_optional_confidence_desc, compare_route_handler_candidates,
    index_freshness_from_storage, indexable_source_path, indexable_source_path_in_workspace,
    not_checked_index_freshness, process_env_test_lock, resolve_project_file_path_from_root,
    stored_file_coverage_diagnostics,
};
use codestory_contracts::graph::{
    Edge, EdgeId, EdgeKind, Node, NodeId as CoreNodeId, NodeKind, Occurrence, OccurrenceKind,
    ResolutionCertainty, SourceLocation,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};
use tempfile::tempdir;

fn all_permutations<T: Clone>(values: &[T]) -> Vec<Vec<T>> {
    fn visit<T: Clone>(values: &mut [T], index: usize, output: &mut Vec<Vec<T>>) {
        if index == values.len() {
            output.push(values.to_vec());
            return;
        }
        for candidate in index..values.len() {
            values.swap(index, candidate);
            visit(values, index + 1, output);
            values.swap(index, candidate);
        }
    }

    let mut values = values.to_vec();
    let mut output = Vec::new();
    visit(&mut values, 0, &mut output);
    output
}

#[test]
fn affected_identity_matching_visits_one_bucket_for_all_same_identity_aliases() {
    let root = tempdir().expect("project");
    let seed = root.path().join("identity-seed.rs");
    fs::write(&seed, "pub fn seed() {}\n").expect("write identity seed");
    let shared_identity =
        codestory_workspace::workspace_path_identity(&seed).expect("shared native identity");
    let change_records = (0..200)
        .map(|index| AffectedChangeRecordDto {
            path: format!("changed-alias-{index}.rs"),
            kind: AffectedChangeKindDto::Renamed,
            status: "R".to_string(),
            previous_path: Some(format!("previous-alias-{index}.rs")),
        })
        .collect::<Vec<_>>();
    let indexed_files = (0..25_000)
        .map(|index| {
            (
                CoreNodeId(index + 1),
                root.path().join(format!("indexed-alias-{index}.rs")),
            )
        })
        .collect::<Vec<_>>();
    let resolver_calls = std::cell::Cell::new(0_usize);
    let mut path_identities = AffectedOperationIdentityIndex::with_resolver(|_: &Path| {
        resolver_calls.set(resolver_calls.get() + 1);
        Ok(shared_identity.clone())
    });
    for (_, path) in &indexed_files {
        path_identities.record_admitted(path);
        path_identities.record_stale(path);
    }

    // The injected identity models 25,000 indexed hardlink spellings and
    // 200 current and previous aliases of the same native file. Refresh
    // membership and affected matching share each indexed observation.
    let matches = match_affected_file_identities(
        root.path(),
        &change_records,
        indexed_files
            .iter()
            .map(|(file_id, path)| (*file_id, path.as_path())),
        &mut path_identities,
    );

    assert_eq!(resolver_calls.get(), 25_400);
    assert_eq!(matches.matched_file_ids.len(), 25_000);
    assert_eq!(
        matches
            .matched_record_flags
            .iter()
            .filter(|matched| **matched)
            .count(),
        200
    );
    assert_eq!(matches.matched_record_index_by_file_id.len(), 25_000);
    assert!(
        matches
            .matched_record_index_by_file_id
            .values()
            .all(|record_index| *record_index == 0)
    );
    assert_eq!(matches.work.record_visits, 200);
    assert_eq!(matches.work.indexed_file_visits, 25_000);
    assert_eq!(matches.work.current_identity_bucket_visits, 1);
    assert_eq!(matches.work.previous_identity_bucket_visits, 1);
    assert!(matches.work.bucket_record_visits <= change_records.len() * 2);
    assert!(matches.work.indexed_bucket_file_visits <= indexed_files.len() * 2);
    assert!(
        matches
            .current_identity_error_by_record
            .iter()
            .all(Option::is_none)
    );
    assert!(
        matches
            .previous_identity_error_by_record
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(matches.unavailable_indexed_identity_count, 0);
}

#[test]
fn affected_identity_matching_uses_native_hardlink_identity() {
    let root = tempdir().expect("project");
    let indexed = root.path().join("indexed.rs");
    let alias = root.path().join("alias.rs");
    fs::write(&indexed, "pub fn indexed() {}\n").expect("write indexed source");
    fs::hard_link(&indexed, &alias).expect("create hardlink alias");
    let records = vec![affected_test_record(
        AffectedChangeKindDto::Modified,
        "alias.rs",
    )];
    let mut identities = AffectedOperationIdentityIndex::native();

    let matches = match_affected_file_identities(
        root.path(),
        &records,
        std::iter::once((CoreNodeId(1), indexed.as_path())),
        &mut identities,
    );

    assert_eq!(matches.matched_file_ids, HashSet::from([CoreNodeId(1)]));
    assert_eq!(matches.matched_record_flags, vec![true]);
    assert_eq!(matches.work.indexed_file_visits, 1);
}

#[test]
fn affected_identity_matching_processes_first_seen_previous_bucket_once() {
    let root = tempdir().expect("project");
    let shared_seed = root.path().join("shared.rs");
    fs::write(&shared_seed, "pub fn shared() {}\n").expect("write shared seed");
    let shared_identity =
        codestory_workspace::workspace_path_identity(&shared_seed).expect("shared identity");
    let distinct_identities = (0..3)
        .map(|index| {
            let path = root.path().join(format!("distinct-{index}.rs"));
            fs::write(&path, format!("pub fn distinct_{index}() {{}}\n"))
                .expect("write distinct seed");
            codestory_workspace::workspace_path_identity(&path).expect("distinct identity")
        })
        .collect::<Vec<_>>();
    let records = (0..3)
        .map(|index| {
            affected_test_move_record(
                AffectedChangeKindDto::Renamed,
                &format!("current-{index}.rs"),
                &format!("previous-{index}.rs"),
            )
        })
        .collect::<Vec<_>>();
    let indexed = [
        (CoreNodeId(30), PathBuf::from("indexed-30.rs")),
        (CoreNodeId(10), PathBuf::from("indexed-10.rs")),
        (CoreNodeId(20), PathBuf::from("indexed-20.rs")),
    ];
    let mut identities = AffectedOperationIdentityIndex::with_resolver({
        let shared_identity = shared_identity.clone();
        move |path: &Path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if let Some(index) = name
                .strip_prefix("current-")
                .and_then(|value| value.strip_suffix(".rs"))
                .and_then(|value| value.parse::<usize>().ok())
            {
                return Ok(distinct_identities[index].clone());
            }
            Ok(shared_identity.clone())
        }
    });

    let matches = match_affected_file_identities(
        root.path(),
        &records,
        indexed
            .iter()
            .map(|(file_id, path)| (*file_id, path.as_path())),
        &mut identities,
    );

    assert!(matches.matched_file_ids.is_empty());
    assert_eq!(matches.graph_seeded_record_flags, vec![true, true, true]);
    assert_eq!(
        matches
            .previous_record_index_by_file_id
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            (CoreNodeId(10), 0),
            (CoreNodeId(20), 0),
            (CoreNodeId(30), 0),
        ])
    );
    assert_eq!(matches.work.record_visits, 3);
    assert_eq!(matches.work.indexed_file_visits, 3);
    assert_eq!(matches.work.current_identity_bucket_visits, 3);
    assert_eq!(matches.work.previous_identity_bucket_visits, 1);
    assert_eq!(matches.work.bucket_record_visits, 3);
    assert_eq!(matches.work.indexed_bucket_file_visits, 3);
}

#[test]
fn affected_identity_matching_is_invariant_to_indexed_input_order() {
    let root = tempdir().expect("project");
    let first_seed = root.path().join("first-seed.rs");
    let second_seed = root.path().join("second-seed.rs");
    fs::write(&first_seed, "pub fn first() {}\n").expect("write first seed");
    fs::write(&second_seed, "pub fn second() {}\n").expect("write second seed");
    let first_identity =
        codestory_workspace::workspace_path_identity(&first_seed).expect("first identity");
    let second_identity =
        codestory_workspace::workspace_path_identity(&second_seed).expect("second identity");
    let records = vec![
        affected_test_record(AffectedChangeKindDto::Modified, "current-first.rs"),
        affected_test_record(AffectedChangeKindDto::Modified, "current-second.rs"),
    ];
    let forward = vec![
        (CoreNodeId(30), PathBuf::from("indexed-first-z.rs")),
        (CoreNodeId(10), PathBuf::from("indexed-first-a.rs")),
        (CoreNodeId(20), PathBuf::from("indexed-second.rs")),
        (CoreNodeId(40), PathBuf::from("missing-z.rs")),
        (CoreNodeId(50), PathBuf::from("missing-a.rs")),
    ];
    let run = |indexed: &[(CoreNodeId, PathBuf)]| {
        let first_identity = first_identity.clone();
        let second_identity = second_identity.clone();
        let mut identities = AffectedOperationIdentityIndex::with_resolver(move |path: &Path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name == "current-first.rs" || name.starts_with("indexed-first-") {
                return Ok(first_identity.clone());
            }
            if matches!(name, "current-second.rs" | "indexed-second.rs") {
                return Ok(second_identity.clone());
            }
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{name} unavailable"),
            ))
        });
        let matches = match_affected_file_identities(
            root.path(),
            &records,
            indexed
                .iter()
                .map(|(file_id, path)| (*file_id, path.as_path())),
            &mut identities,
        );
        (
            matches.matched_file_ids,
            matches.matched_record_flags,
            matches.matched_record_index_by_file_id,
            matches.graph_seeded_record_flags,
            matches.unavailable_indexed_identity_count,
            matches.unavailable_indexed_identity_sample,
            matches.work,
        )
    };

    let mut reverse = forward.clone();
    reverse.reverse();
    let mut rotated = forward.clone();
    rotated.rotate_left(2);
    let expected = run(&forward);
    assert_eq!(run(&reverse), expected);
    assert_eq!(run(&rotated), expected);
    assert_eq!(expected.4, 2);
    assert!(
        expected
            .5
            .as_deref()
            .is_some_and(|sample| sample.contains("missing-a.rs unavailable"))
    );
}

#[test]
fn affected_identity_matching_counts_actual_two_phase_worst_case_work() {
    let root = tempdir().expect("project");
    let shared_seed = root.path().join("shared-seed.rs");
    let previous_seed = root.path().join("previous-seed.rs");
    let current_seed = root.path().join("current-seed.rs");
    for path in [&shared_seed, &previous_seed, &current_seed] {
        fs::write(path, "pub fn seed() {}\n").expect("write identity seed");
    }
    let shared_identity =
        codestory_workspace::workspace_path_identity(&shared_seed).expect("shared identity");
    let previous_identity =
        codestory_workspace::workspace_path_identity(&previous_seed).expect("previous identity");
    let current_identity =
        codestory_workspace::workspace_path_identity(&current_seed).expect("current identity");
    let records = vec![
        affected_test_move_record(
            AffectedChangeKindDto::Renamed,
            "current-matched.rs",
            "previous-unmatched.rs",
        ),
        affected_test_move_record(
            AffectedChangeKindDto::Renamed,
            "current-unmatched.rs",
            "previous-matched.rs",
        ),
    ];
    let indexed = (0..1_000)
        .map(|index| {
            (
                CoreNodeId(index + 1),
                PathBuf::from(format!("indexed-{index}.rs")),
            )
        })
        .collect::<Vec<_>>();
    let resolver_calls = std::cell::Cell::new(0_usize);
    let resolver_calls_for_run = &resolver_calls;
    let mut identities = AffectedOperationIdentityIndex::with_resolver({
        let shared_identity = shared_identity.clone();
        move |path: &Path| {
            resolver_calls_for_run.set(resolver_calls_for_run.get() + 1);
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name == "current-matched.rs"
                || name == "previous-matched.rs"
                || name.starts_with("indexed-")
            {
                Ok(shared_identity.clone())
            } else if name == "previous-unmatched.rs" {
                Ok(previous_identity.clone())
            } else {
                Ok(current_identity.clone())
            }
        }
    });

    let matches = match_affected_file_identities(
        root.path(),
        &records,
        indexed
            .iter()
            .map(|(file_id, path)| (*file_id, path.as_path())),
        &mut identities,
    );

    assert_eq!(resolver_calls.get(), indexed.len() + records.len() * 2);
    assert_eq!(matches.matched_file_ids.len(), indexed.len());
    assert_eq!(matches.matched_record_flags, vec![true, false]);
    assert_eq!(matches.graph_seeded_record_flags, vec![true, true]);
    assert!(matches.previous_record_index_by_file_id.is_empty());
    assert_eq!(matches.work.record_visits, records.len());
    assert_eq!(matches.work.indexed_file_visits, indexed.len());
    assert_eq!(matches.work.bucket_record_visits, records.len());
    assert_eq!(matches.work.indexed_bucket_file_visits, indexed.len() * 2);
    assert!(matches.work.bucket_record_visits <= records.len() * 2);
    assert!(matches.work.indexed_bucket_file_visits <= indexed.len() * 2);
}

#[test]
fn affected_identity_matching_reports_unavailable_observations() {
    let root = tempdir().expect("project");
    let change_records = vec![AffectedChangeRecordDto {
        path: "identity-unavailable.rs".to_string(),
        kind: AffectedChangeKindDto::Modified,
        status: "M".to_string(),
        previous_path: None,
    }];
    let indexed_path = PathBuf::from("identity-unavailable.rs");
    let resolver_calls = std::cell::Cell::new(0_usize);
    let mut path_identities = AffectedOperationIdentityIndex::with_resolver(|_: &Path| {
        resolver_calls.set(resolver_calls.get() + 1);
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "injected identity failure",
        ))
    });

    let matches = match_affected_file_identities(
        root.path(),
        &change_records,
        std::iter::once((CoreNodeId(1), indexed_path.as_path())),
        &mut path_identities,
    );

    assert_eq!(resolver_calls.get(), 1);
    assert!(matches.matched_file_ids.is_empty());
    assert_eq!(matches.matched_record_flags, vec![false]);
    assert_eq!(
        matches
            .current_identity_error_by_record
            .iter()
            .filter(|error| error.is_some())
            .count(),
        1
    );
    assert_eq!(matches.unavailable_indexed_identity_count, 1);
    assert!(
        matches.current_identity_error_by_record[0]
            .as_deref()
            .is_some_and(|sample| sample.contains("injected identity failure"))
    );
    assert!(
        matches
            .unavailable_indexed_identity_sample
            .as_deref()
            .is_some_and(|sample| sample.contains("injected identity failure"))
    );
}

#[test]
fn affected_identity_matching_accounts_current_previous_and_indexed_failures_independently() {
    let root = tempdir().expect("project");
    let shared_seed = root.path().join("shared-seed.rs");
    let distinct_seed = root.path().join("distinct-seed.rs");
    fs::write(&shared_seed, "pub fn shared() {}\n").expect("write shared seed");
    fs::write(&distinct_seed, "pub fn distinct() {}\n").expect("write distinct seed");
    let shared_identity =
        codestory_workspace::workspace_path_identity(&shared_seed).expect("shared identity");
    let distinct_identity =
        codestory_workspace::workspace_path_identity(&distinct_seed).expect("distinct identity");
    let records = vec![affected_test_move_record(
        AffectedChangeKindDto::Renamed,
        "current.rs",
        "previous.rs",
    )];
    let indexed = PathBuf::from("indexed.rs");

    let mut current_failure = AffectedOperationIdentityIndex::with_resolver({
        let shared_identity = shared_identity.clone();
        move |path: &Path| {
            if path.ends_with("current.rs") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "current identity failure",
                ))
            } else {
                Ok(shared_identity.clone())
            }
        }
    });
    let current = match_affected_file_identities(
        root.path(),
        &records,
        std::iter::once((CoreNodeId(1), indexed.as_path())),
        &mut current_failure,
    );
    assert!(
        current.current_identity_error_by_record[0]
            .as_deref()
            .is_some_and(|error| error.contains("current identity failure"))
    );
    assert_eq!(current.matched_record_flags, vec![false]);
    assert_eq!(current.graph_seeded_record_flags, vec![true]);

    let mut previous_failure = AffectedOperationIdentityIndex::with_resolver({
        let shared_identity = shared_identity.clone();
        let distinct_identity = distinct_identity.clone();
        move |path: &Path| {
            if path.ends_with("previous.rs") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "previous identity failure",
                ))
            } else if path.ends_with("current.rs") {
                Ok(distinct_identity.clone())
            } else {
                Ok(shared_identity.clone())
            }
        }
    });
    let previous = match_affected_file_identities(
        root.path(),
        &records,
        std::iter::once((CoreNodeId(1), indexed.as_path())),
        &mut previous_failure,
    );
    assert!(
        previous.previous_identity_error_by_record[0]
            .as_deref()
            .is_some_and(|error| error.contains("previous identity failure"))
    );
    assert_eq!(previous.matched_record_flags, vec![false]);
    assert_eq!(previous.graph_seeded_record_flags, vec![false]);

    let mut indexed_failure = AffectedOperationIdentityIndex::with_resolver({
        let shared_identity = shared_identity.clone();
        move |path: &Path| {
            if path.ends_with("indexed.rs") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "indexed identity failure",
                ))
            } else {
                Ok(shared_identity.clone())
            }
        }
    });
    let indexed_result = match_affected_file_identities(
        root.path(),
        &records,
        std::iter::once((CoreNodeId(1), indexed.as_path())),
        &mut indexed_failure,
    );
    assert_eq!(indexed_result.unavailable_indexed_identity_count, 1);
    assert!(
        indexed_result
            .unavailable_indexed_identity_sample
            .as_deref()
            .is_some_and(|error| error.contains("indexed identity failure"))
    );
    assert_eq!(indexed_result.matched_record_flags, vec![false]);

    let mut excludable_previous = AffectedOperationIdentityIndex::with_resolver({
        let shared_identity = shared_identity.clone();
        move |path: &Path| {
            if path.ends_with("previous.rs") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "excludable previous identity failure",
                ))
            } else {
                Ok(shared_identity.clone())
            }
        }
    });
    let excludable = match_affected_file_identities(
        root.path(),
        &records,
        std::iter::once((CoreNodeId(1), indexed.as_path())),
        &mut excludable_previous,
    );
    assert_eq!(excludable.matched_record_flags, vec![true]);
    assert!(excludable.previous_identity_error_by_record[0].is_some());
}

#[test]
fn affected_gap_composition_has_zero_one_hot_and_all_four_controls() {
    let completeness_for = |gap_composition| {
        compose_affected_completeness(AffectedCompletenessInput {
            uncovered_input_count: 0,
            direct_impact_count: 1,
            propagated_impact_count: 2,
            candidate_test_count: 3,
            freshness_evidence_affects_requested_claim: false,
            gap_composition,
            truncation_reasons: Vec::new(),
        })
    };
    let zero = compose_affected_evidence_gaps(0, &AffectedRelevantEvidenceGaps::default());
    assert!(zero.gap_free);
    assert_eq!(zero.confidence(), "complete");
    assert_eq!(zero.unavailable_evidence_count, 0);
    assert!(zero.blind_spots.is_empty());
    let zero_completeness = completeness_for(zero);
    assert!(zero_completeness.complete);
    assert_eq!(zero_completeness.confidence, "complete");
    assert_eq!(zero_completeness.unavailable_evidence_count, 0);

    for category in ["current", "previous", "indexed", "freshness"] {
        let mut gaps = AffectedRelevantEvidenceGaps::default();
        let gap = match category {
            "current" => &mut gaps.current,
            "previous" => &mut gaps.previous,
            "indexed" => &mut gaps.indexed,
            "freshness" => &mut gaps.freshness,
            _ => unreachable!(),
        };
        gap.count = 1;
        gap.sample = Some(format!("{category} sample"));
        let composition = compose_affected_evidence_gaps(0, &gaps);
        assert!(!composition.gap_free, "{category}");
        assert_eq!(composition.confidence(), "bounded", "{category}");
        assert_eq!(composition.unavailable_evidence_count, 1, "{category}");
        assert_eq!(composition.blind_spots.len(), 1, "{category}");
        assert!(composition.blind_spots[0].contains(category), "{category}");
        let completeness = completeness_for(composition);
        assert!(!completeness.complete, "{category}");
        assert_eq!(completeness.confidence, "bounded", "{category}");
        assert_eq!(completeness.unavailable_evidence_count, 1, "{category}");
    }

    let all_four = AffectedRelevantEvidenceGaps {
        current: AffectedEvidenceGapCategory {
            count: 1,
            sample: Some("current sample".to_string()),
        },
        previous: AffectedEvidenceGapCategory {
            count: 2,
            sample: Some("previous sample".to_string()),
        },
        indexed: AffectedEvidenceGapCategory {
            count: 3,
            sample: Some("indexed sample".to_string()),
        },
        freshness: AffectedEvidenceGapCategory {
            count: 4,
            sample: Some("freshness sample".to_string()),
        },
    };
    let composition = compose_affected_evidence_gaps(0, &all_four);
    assert!(!composition.gap_free);
    assert_eq!(composition.confidence(), "bounded");
    assert_eq!(composition.unavailable_evidence_count, 10);
    assert_eq!(composition.blind_spots.len(), 4);
    for (blind_spot, category) in composition.blind_spots.iter().zip([
        "current-path",
        "previous-path",
        "indexed files",
        "refresh-plan",
    ]) {
        assert!(blind_spot.contains(category), "{blind_spot}");
    }
    let completeness = completeness_for(composition);
    assert!(!completeness.complete);
    assert_eq!(completeness.confidence, "bounded");
    assert_eq!(completeness.unavailable_evidence_count, 10);
}

#[test]
fn affected_gap_relevance_is_pure_and_excludes_previous_after_current_match() {
    let resolved = vec![AffectedResolvedInput {
        current: PathBuf::from("current.rs"),
        previous: Some(PathBuf::from("previous.rs")),
    }];
    let current_errors = vec![Some("current gap".to_string())];
    let previous_errors = vec![Some("previous gap".to_string())];
    let unmatched = [false];
    let all_relevant = affected_relevant_evidence_gaps(AffectedRelevantEvidenceGapInput {
        workspace_root: None,
        resolved_inputs: &resolved,
        matched_record_flags: &unmatched,
        current_identity_errors: &current_errors,
        previous_identity_errors: &previous_errors,
        unavailable_indexed_identity_count: 2,
        unavailable_indexed_identity_sample: Some("indexed gap"),
        freshness_evidence_affects_requested_claim: true,
        freshness_identity_gap_count: 3,
        freshness_identity_gap_sample: Some("freshness gap"),
    });
    assert_eq!(all_relevant.current.count, 1);
    assert_eq!(all_relevant.previous.count, 1);
    assert_eq!(all_relevant.indexed.count, 2);
    assert_eq!(all_relevant.freshness.count, 3);

    let matched = [true];
    let current_wins = affected_relevant_evidence_gaps(AffectedRelevantEvidenceGapInput {
        workspace_root: None,
        resolved_inputs: &resolved,
        matched_record_flags: &matched,
        current_identity_errors: &current_errors,
        previous_identity_errors: &previous_errors,
        unavailable_indexed_identity_count: 0,
        unavailable_indexed_identity_sample: None,
        freshness_evidence_affects_requested_claim: false,
        freshness_identity_gap_count: 0,
        freshness_identity_gap_sample: None,
    });
    assert_eq!(current_wins.current.count, 1);
    assert_eq!(current_wins.previous.count, 0);

    let svg = vec![AffectedResolvedInput {
        current: PathBuf::from("desk.svg"),
        previous: None,
    }];
    let irrelevant = affected_relevant_evidence_gaps(AffectedRelevantEvidenceGapInput {
        workspace_root: None,
        resolved_inputs: &svg,
        matched_record_flags: &[false],
        current_identity_errors: &current_errors,
        previous_identity_errors: &[None],
        unavailable_indexed_identity_count: 4,
        unavailable_indexed_identity_sample: Some("irrelevant indexed gap"),
        freshness_evidence_affects_requested_claim: false,
        freshness_identity_gap_count: 5,
        freshness_identity_gap_sample: Some("irrelevant freshness gap"),
    });
    assert_eq!(irrelevant, AffectedRelevantEvidenceGaps::default());
}

pub(crate) struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    pub(crate) fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    pub(crate) fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(value) = self.previous.as_deref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

#[test]
fn default_runtime_source_policy_does_not_read_process_environment() {
    let _lock = process_env_test_lock();
    let _cap = EnvGuard::set("CODESTORY_INDEX_SOURCE_FILE_BYTE_CAP", "17");

    let controller = AppController::new();

    assert_eq!(
        controller.source_index_policy.as_ref(),
        &SourceIndexPolicy::default()
    );
}

pub(crate) fn assert_mandatory_retrieval_unavailable(error: &ApiError) {
    assert_eq!(error.code, "retrieval_unavailable");
    assert!(
        error
            .message
            .contains("retrieval is unavailable or degraded")
            || error.message.contains("full retrieval is mandatory"),
        "expected mandatory retrieval failure, got {error:?}"
    );
    let details = error.details.as_ref().expect("retrieval error details");
    assert_eq!(details.failed_layer.as_deref(), Some("retrieval_engine"));
    assert!(
        !details.next_commands.is_empty(),
        "retrieval error should include recovery commands: {error:?}"
    );
}

fn affected_test_freshness(
    root: &Path,
    status: IndexFreshnessStatusDto,
    samples: Vec<IndexFreshnessSampleDto>,
) -> IndexFreshnessObservation {
    let stale_paths = samples
        .iter()
        .map(|sample| root.join(&sample.path))
        .collect::<Vec<_>>();
    let stale_identities = stale_paths
        .iter()
        .map(|path| {
            codestory_workspace::workspace_path_identity(path)
                .expect("test freshness path identity")
        })
        .collect::<HashSet<_>>();
    IndexFreshnessObservation {
        freshness: IndexFreshnessDto {
            status,
            changed_file_count: 0,
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 1,
            indexed_file_count: 1,
            duration_ms: 0,
            reason: None,
            samples,
        },
        inventory_complete: status != IndexFreshnessStatusDto::NotChecked,
        admitted_identities: stale_identities.clone(),
        stale_identities,
        identity_gap_count: 0,
        identity_gap_sample: None,
    }
}

fn classify_unmatched_affected_input_for_test(
    record: &AffectedChangeRecordDto,
    resolved: &AffectedResolvedInput,
    freshness: &IndexFreshnessObservation,
) -> (AffectedInputClassificationDto, String, Vec<String>) {
    let current_identity = codestory_workspace::workspace_path_identity(&resolved.current).ok();
    classify_unmatched_affected_input(
        None,
        record,
        resolved,
        freshness,
        current_identity.as_ref(),
        None,
        None,
    )
}

fn affected_test_record(kind: AffectedChangeKindDto, path: &str) -> AffectedChangeRecordDto {
    AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: "test".to_string(),
        previous_path: None,
    }
}

fn affected_test_move_record(
    kind: AffectedChangeKindDto,
    path: &str,
    previous_path: &str,
) -> AffectedChangeRecordDto {
    AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: "test".to_string(),
        previous_path: Some(previous_path.to_string()),
    }
}

#[test]
fn affected_runtime_normalization_rejects_previous_path_for_non_move_records() {
    let error = normalized_affected_input(&AffectedAnalysisInput::ChangeRecords(vec![
        affected_test_move_record(AffectedChangeKindDto::Modified, "new.rs", "old.rs"),
    ]))
    .expect_err("modified records must not carry previous identity");

    assert_eq!(error.code, "invalid_argument");
    assert!(error.message.contains("renamed or copied"));
}

#[test]
fn affected_unmatched_classification_uses_positive_path_and_freshness_evidence() {
    let project = tempdir().expect("project");
    let svg = project.path().join("desk.svg");
    fs::write(&svg, "<svg/>").expect("write svg");
    let fresh = affected_test_freshness(project.path(), IndexFreshnessStatusDto::Fresh, Vec::new());
    let resolved_svg = AffectedResolvedInput {
        current: svg.clone(),
        previous: None,
    };
    let (classification, _, _) = classify_unmatched_affected_input_for_test(
        &affected_test_record(AffectedChangeKindDto::Modified, "desk.svg"),
        &resolved_svg,
        &fresh,
    );
    assert_eq!(
        classification,
        AffectedInputClassificationDto::ValidUncovered
    );

    let source = project.path().join("new.rs");
    fs::write(&source, "pub fn new_source() {}\n").expect("write source");
    let stale = affected_test_freshness(
        project.path(),
        IndexFreshnessStatusDto::Stale,
        vec![IndexFreshnessSampleDto {
            kind: IndexFreshnessChangeKindDto::New,
            path: "new.rs".to_string(),
        }],
    );
    let resolved_source = AffectedResolvedInput {
        current: source,
        previous: None,
    };
    let (classification, _, evidence) = classify_unmatched_affected_input_for_test(
        &affected_test_record(AffectedChangeKindDto::Added, "new.rs"),
        &resolved_source,
        &stale,
    );
    assert_eq!(classification, AffectedInputClassificationDto::StaleIndex);
    assert!(
        evidence
            .iter()
            .any(|entry| entry.contains("freshness evidence"))
    );
}

#[test]
fn affected_unmatched_classification_distinguishes_missing_delete_and_rename() {
    let project = tempdir().expect("project");
    fs::create_dir(project.path().join("src")).expect("create source directory");
    let missing =
        resolve_project_file_path_from_root(project.path(), "src/deleted/parents/missing.rs", true)
            .expect("resolve nested missing current path");
    let previous = resolve_project_file_path_from_root(
        project.path(),
        "src/old/deleted/parents/previous.rs",
        true,
    )
    .expect("resolve nested missing previous path");
    let resolved = AffectedResolvedInput {
        current: missing,
        previous: Some(previous),
    };
    let freshness =
        affected_test_freshness(project.path(), IndexFreshnessStatusDto::Fresh, Vec::new());
    for (kind, expected) in [
        (
            AffectedChangeKindDto::Modified,
            AffectedInputClassificationDto::Missing,
        ),
        (
            AffectedChangeKindDto::Deleted,
            AffectedInputClassificationDto::ExpectedDeleted,
        ),
        (
            AffectedChangeKindDto::Renamed,
            AffectedInputClassificationDto::RenameUnresolved,
        ),
    ] {
        let (classification, _, _) = classify_unmatched_affected_input_for_test(
            &affected_test_record(kind, "src/deleted/parents/missing.rs"),
            &resolved,
            &freshness,
        );
        assert_eq!(classification, expected);
    }
}

#[test]
fn affected_unmatched_directory_is_malformed_not_valid_uncovered() {
    let project = tempdir().expect("project");
    let directory = project.path().join("assets");
    fs::create_dir(&directory).expect("create directory");
    let resolved = AffectedResolvedInput {
        current: directory,
        previous: None,
    };
    let freshness =
        affected_test_freshness(project.path(), IndexFreshnessStatusDto::Fresh, Vec::new());

    let (classification, reason, _) = classify_unmatched_affected_input_for_test(
        &affected_test_record(AffectedChangeKindDto::Modified, "assets"),
        &resolved,
        &freshness,
    );

    assert_eq!(classification, AffectedInputClassificationDto::Malformed);
    assert!(reason.contains("regular file"));
}

#[test]
fn affected_unmatched_metadata_error_is_unavailable_not_missing() {
    let project = tempdir().expect("project");
    let resolved = AffectedResolvedInput {
        current: project.path().join("unreadable.rs"),
        previous: None,
    };
    let freshness =
        affected_test_freshness(project.path(), IndexFreshnessStatusDto::Fresh, Vec::new());

    let (classification, reason, evidence) = classify_unmatched_affected_input_with_metadata(
        &affected_test_record(AffectedChangeKindDto::Modified, "unreadable.rs"),
        &resolved,
        &freshness,
        None,
        None,
        None,
        AffectedUnmatchedPathObservation {
            workspace_root: None,
            metadata: AffectedPathMetadataObservation::Unavailable {
                kind: io::ErrorKind::PermissionDenied,
                message: "injected metadata denial".to_string(),
            },
        },
    );

    assert_eq!(
        classification,
        AffectedInputClassificationDto::UnavailableEvidence
    );
    assert!(reason.contains("metadata was unavailable"));
    assert!(evidence.iter().any(|item| {
        item.contains("kind=PermissionDenied") && item.contains("injected metadata denial")
    }));
    assert!(evidence.iter().all(|item| !item.contains("missing")));
}

#[cfg(unix)]
#[test]
fn affected_unmatched_broken_symlink_is_malformed_not_missing() {
    let project = tempdir().expect("project");
    let broken_link = project.path().join("broken.rs");
    std::os::unix::fs::symlink(project.path().join("absent-target.rs"), &broken_link)
        .expect("create broken symlink");
    let resolved = AffectedResolvedInput {
        current: broken_link,
        previous: None,
    };
    let freshness =
        affected_test_freshness(project.path(), IndexFreshnessStatusDto::Fresh, Vec::new());

    let (classification, reason, _) = classify_unmatched_affected_input_for_test(
        &affected_test_record(AffectedChangeKindDto::Modified, "broken.rs"),
        &resolved,
        &freshness,
    );

    assert_eq!(classification, AffectedInputClassificationDto::Malformed);
    assert!(reason.contains("regular file"));
}

#[test]
fn affected_matched_recorded_error_takes_precedence_over_stale_evidence() {
    let mut file = AffectedMatchedFileDto {
        path: "src/lib.rs".to_string(),
        role: IndexedFileRoleDto::Source,
        indexed: true,
        complete: false,
        change_kind: Some(AffectedChangeKindDto::Modified),
        change_status: Some("M".to_string()),
        previous_path: None,
        error_count: 1,
    };

    let (classification, _, evidence) =
        classify_matched_affected_input(&file, true, true).expect("incomplete evidence");
    assert_eq!(classification, AffectedInputClassificationDto::Malformed);
    assert!(evidence.iter().any(|item| item.contains("error_count=1")));

    file.error_count = 0;
    let (classification, _, _) =
        classify_matched_affected_input(&file, true, true).expect("stale evidence");
    assert_eq!(classification, AffectedInputClassificationDto::StaleIndex);
}

#[test]
fn affected_indexable_absence_uses_complete_inventory_and_exact_staleness() {
    let project = tempdir().expect("project");
    let excluded = project.path().join("target/excluded.rs");
    fs::create_dir_all(excluded.parent().expect("excluded parent")).expect("create target");
    fs::write(&excluded, "pub fn excluded() {}\n").expect("write excluded source");
    let resolved = AffectedResolvedInput {
        current: excluded.clone(),
        previous: None,
    };
    let record = affected_test_record(AffectedChangeKindDto::Modified, "target/excluded.rs");
    let admitted_identity =
        codestory_workspace::workspace_path_identity(&project.path().join("src/lib.rs"))
            .expect("admitted identity");
    let unrelated_stale_identity =
        codestory_workspace::workspace_path_identity(&project.path().join("src/unrelated.rs"))
            .expect("unrelated stale identity");

    let complete_excluded = IndexFreshnessObservation {
        freshness: affected_test_freshness(
            project.path(),
            IndexFreshnessStatusDto::Stale,
            Vec::new(),
        )
        .freshness,
        inventory_complete: true,
        admitted_identities: HashSet::from([admitted_identity]),
        stale_identities: HashSet::from([unrelated_stale_identity]),
        identity_gap_count: 0,
        identity_gap_sample: None,
    };
    let (classification, _, evidence) =
        classify_unmatched_affected_input_for_test(&record, &resolved, &complete_excluded);
    assert_eq!(
        classification,
        AffectedInputClassificationDto::ValidUncovered
    );
    assert!(
        evidence
            .iter()
            .any(|item| item.contains("inventory excludes"))
    );

    let excluded_identity =
        codestory_workspace::workspace_path_identity(&excluded).expect("excluded identity");
    let exact_stale = IndexFreshnessObservation {
        admitted_identities: HashSet::from([excluded_identity.clone()]),
        stale_identities: HashSet::from([excluded_identity]),
        ..complete_excluded.clone()
    };
    let (classification, _, _) =
        classify_unmatched_affected_input_for_test(&record, &resolved, &exact_stale);
    assert_eq!(classification, AffectedInputClassificationDto::StaleIndex);

    let incomplete = IndexFreshnessObservation::incomplete(not_checked_index_freshness(
        "bounded inventory",
        1,
        Instant::now(),
    ));
    let (classification, _, _) =
        classify_unmatched_affected_input_for_test(&record, &resolved, &incomplete);
    assert_eq!(
        classification,
        AffectedInputClassificationDto::UnavailableEvidence
    );
}

#[test]
fn affected_refresh_follow_up_requires_requested_stale_classification() {
    let unrelated_stale = AffectedUncoveredInputDto {
        path: "target/excluded.rs".to_string(),
        classification: AffectedInputClassificationDto::ValidUncovered,
        reason: "excluded by complete inventory".to_string(),
        evidence: Vec::new(),
    };
    let follow_ups = affected_follow_ups("/project", &[unrelated_stale], None);
    assert!(
        follow_ups
            .iter()
            .all(|follow_up| follow_up.action != "refresh_stale_index")
    );

    let requested_stale = AffectedUncoveredInputDto {
        path: "src/lib.rs".to_string(),
        classification: AffectedInputClassificationDto::StaleIndex,
        reason: "exact stale path".to_string(),
        evidence: Vec::new(),
    };
    let follow_ups = affected_follow_ups("/project", &[requested_stale], None);
    assert_eq!(
        follow_ups
            .iter()
            .filter(|follow_up| follow_up.action == "refresh_stale_index")
            .count(),
        1
    );
}

#[test]
fn affected_follow_ups_are_deduplicated_and_empty_for_complete_input() {
    let duplicate = AffectedUncoveredInputDto {
        path: "desk.svg".to_string(),
        classification: AffectedInputClassificationDto::ValidUncovered,
        reason: "outside graph coverage".to_string(),
        evidence: Vec::new(),
    };

    let follow_ups = affected_follow_ups("/project", &[duplicate.clone(), duplicate], None);

    assert_eq!(follow_ups.len(), 1);
    assert_eq!(follow_ups[0].action, "inspect_graph_boundary");
    assert!(follow_ups[0].invocation.is_none());
    assert!(affected_follow_ups("/project", &[], None).is_empty());
}

#[test]
fn affected_not_checked_svg_has_no_doctor_or_index_follow_up() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("desk.svg"), "<svg/>\n").expect("write SVG");
    Storage::open(project.path().join("codestory.db")).expect("create empty storage");
    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: project.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let result = controller
        .affected_analysis(AffectedAnalysisRequest {
            input: AffectedAnalysisInput::Paths(vec!["desk.svg".to_string()]),
            depth: Some(1),
            filter: None,
        })
        .expect("analyze SVG without freshness inventory");

    assert_eq!(
        result.uncovered_inputs[0].classification,
        AffectedInputClassificationDto::ValidUncovered
    );
    assert_eq!(result.follow_ups.len(), 1);
    assert_eq!(result.follow_ups[0].action, "inspect_graph_boundary");
    assert!(result.follow_ups[0].invocation.is_none());
}

#[test]
fn affected_unrelated_stale_file_does_not_downgrade_fresh_requested_identity() {
    let project = tempdir().expect("project");
    let source_dir = project.path().join("src");
    fs::create_dir(&source_dir).expect("create source directory");
    let requested = source_dir.join("requested.rs");
    let unrelated = source_dir.join("unrelated.rs");
    fs::write(&requested, "pub fn requested() {}\n").expect("write requested source");
    fs::write(&unrelated, "pub fn unrelated() {}\n").expect("write unrelated source");
    let requested_mtime = fs::metadata(&requested)
        .expect("requested metadata")
        .modified()
        .expect("requested mtime")
        .duration_since(UNIX_EPOCH)
        .expect("requested mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;
    {
        let mut storage =
            Storage::open(project.path().join("codestory.db")).expect("open stale fixture storage");
        for file in [
            FileInfo {
                id: 1,
                path: requested.clone(),
                language: "rust".to_string(),
                modification_time: requested_mtime,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            },
            FileInfo {
                id: 2,
                path: unrelated.clone(),
                language: "rust".to_string(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            },
        ] {
            storage.insert_file(&file).expect("insert fixture file");
        }
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: requested.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "requested".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FILE,
                    serialized_name: unrelated.to_string_lossy().to_string(),
                    ..Default::default()
                },
            ])
            .expect("insert fixture nodes");
    }
    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: project.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let result = controller
        .affected_analysis(AffectedAnalysisRequest {
            input: AffectedAnalysisInput::Paths(vec!["src/requested.rs".to_string()]),
            depth: Some(1),
            filter: None,
        })
        .expect("analyze fresh requested path");

    assert!(result.completeness.complete, "{result:?}");
    assert!(result.follow_ups.is_empty(), "{result:?}");
    assert!(
        result
            .blind_spots
            .iter()
            .any(|spot| spot.contains("unrelated stale index state"))
    );
}

#[test]
fn affected_rename_and_copy_classify_current_path_while_previous_identity_only_seeds_graph() {
    let project = tempdir().expect("project");
    let source_dir = project.path().join("src");
    fs::create_dir_all(&source_dir).expect("create source directory");
    let previous_path = source_dir.join("old.rs");
    fs::write(&previous_path, "pub fn previous_seed() {}\n").expect("write previous source");
    let modification_time = fs::metadata(&previous_path)
        .expect("previous metadata")
        .modified()
        .expect("previous mtime")
        .duration_since(UNIX_EPOCH)
        .expect("mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let storage_path = project.path().join("codestory.db");
    {
        let mut storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: previous_path.clone(),
                language: "rust".to_string(),
                modification_time,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert previous file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: previous_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "previous_seed".to_string(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert previous graph");
    }
    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: project.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    for (kind, current_name) in [
        (AffectedChangeKindDto::Renamed, "renamed.rs"),
        (AffectedChangeKindDto::Copied, "copied.rs"),
    ] {
        let current_path = source_dir.join(current_name);
        fs::write(&current_path, "pub fn current_source() {}\n").expect("write current source");
        let result = controller
            .affected_analysis(AffectedAnalysisRequest {
                input: AffectedAnalysisInput::ChangeRecords(vec![affected_test_move_record(
                    kind,
                    &format!("src/{current_name}"),
                    "src/old.rs",
                )]),
                depth: Some(2),
                filter: None,
            })
            .expect("analyze current move path");

        assert_eq!(result.matched_file_count, 0);
        assert!(result.matched_files.is_empty());
        assert_eq!(result.unmatched_paths.len(), 1);
        assert_eq!(
            result.unmatched_paths[0].classification,
            AffectedInputClassificationDto::StaleIndex
        );
        assert_eq!(
            result
                .follow_ups
                .iter()
                .filter(|follow_up| follow_up.action == "refresh_stale_index")
                .count(),
            1
        );
        assert!(
            result
                .blind_spots
                .iter()
                .all(|blind_spot| !blind_spot.contains("unrelated stale index state"))
        );
        let proxy_symbol = result
            .impacted_symbols
            .iter()
            .find(|symbol| symbol.display_name == "previous_seed")
            .expect("previous identity proxy symbol");
        assert_eq!(proxy_symbol.confidence, "bounded");
        assert!(proxy_symbol.reason.contains("previous indexed identity"));
        fs::remove_file(current_path).expect("remove current source");
    }

    let svg_path = project.path().join("desk.svg");
    fs::write(&svg_path, "<svg/>\n").expect("write svg");
    let result = controller
        .affected_analysis(AffectedAnalysisRequest {
            input: AffectedAnalysisInput::ChangeRecords(vec![affected_test_move_record(
                AffectedChangeKindDto::Copied,
                "desk.svg",
                "src/old.rs",
            )]),
            depth: Some(2),
            filter: None,
        })
        .expect("analyze copied static asset");

    assert_eq!(result.matched_file_count, 0);
    assert_eq!(
        result.unmatched_paths[0].classification,
        AffectedInputClassificationDto::ValidUncovered
    );
    assert_eq!(result.follow_ups.len(), 1);
    assert_eq!(result.follow_ups[0].action, "inspect_graph_boundary");
    assert!(
        result
            .follow_ups
            .iter()
            .all(|follow_up| follow_up.action != "refresh_stale_index")
    );
    let proxy_symbol = result
        .impacted_symbols
        .iter()
        .find(|symbol| symbol.display_name == "previous_seed")
        .expect("static copy retains bounded proxy seed");
    assert_eq!(proxy_symbol.confidence, "bounded");

    let current_path = source_dir.join("current.rs");
    fs::write(&current_path, "pub fn current_seed() {}\n").expect("write indexed current");
    let current_mtime = fs::metadata(&current_path)
        .expect("current metadata")
        .modified()
        .expect("current mtime")
        .duration_since(UNIX_EPOCH)
        .expect("mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;
    {
        let mut storage = Storage::open(&storage_path).expect("reopen storage");
        storage
            .insert_file(&FileInfo {
                id: 3,
                path: current_path.clone(),
                language: "rust".to_string(),
                modification_time: current_mtime,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert current file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::FILE,
                    serialized_name: current_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(4),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "current_seed".to_string(),
                    file_node_id: Some(CoreNodeId(3)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert current graph");
    }
    let result = controller
        .affected_analysis(AffectedAnalysisRequest {
            input: AffectedAnalysisInput::ChangeRecords(vec![affected_test_move_record(
                AffectedChangeKindDto::Copied,
                "src/current.rs",
                "src/old.rs",
            )]),
            depth: Some(2),
            filter: None,
        })
        .expect("analyze indexed current identity");
    assert_eq!(result.matched_file_count, 1);
    assert!(
        result
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.display_name == "current_seed" && symbol.confidence == "direct")
    );
    assert!(
        result
            .impacted_symbols
            .iter()
            .all(|symbol| symbol.display_name != "previous_seed")
    );
}

#[test]
fn affected_reverse_walk_is_total_across_every_edge_and_seed_permutation() {
    let edges = vec![
        Edge {
            id: EdgeId(100),
            source: CoreNodeId(30),
            target: CoreNodeId(10),
            kind: EdgeKind::CALL,
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        },
        Edge {
            id: EdgeId(101),
            source: CoreNodeId(30),
            target: CoreNodeId(11),
            kind: EdgeKind::CALL,
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        },
        Edge {
            id: EdgeId(102),
            source: CoreNodeId(30),
            target: CoreNodeId(20),
            kind: EdgeKind::CALL,
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        },
        Edge {
            id: EdgeId(201),
            source: CoreNodeId(40),
            target: CoreNodeId(30),
            kind: EdgeKind::CALL,
            certainty: Some(ResolutionCertainty::Uncertain),
            ..Default::default()
        },
        Edge {
            id: EdgeId(301),
            source: CoreNodeId(50),
            target: CoreNodeId(20),
            kind: EdgeKind::CALL,
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        },
    ];
    let seeds = vec![
        (
            CoreNodeId(10),
            AffectedGraphEvidence::seed(
                CoreNodeId(10),
                "first direct seed",
                AffectedConfidenceFloor::from_label("direct"),
                false,
            ),
        ),
        (
            CoreNodeId(11),
            AffectedGraphEvidence::seed(
                CoreNodeId(11),
                "second direct seed",
                AffectedConfidenceFloor::from_label("direct"),
                false,
            ),
        ),
        (
            CoreNodeId(20),
            AffectedGraphEvidence::seed(
                CoreNodeId(20),
                "previous identity proxy seed",
                AffectedConfidenceFloor::from_label("direct"),
                true,
            ),
        ),
    ];
    let labels = HashMap::from([
        (CoreNodeId(10), "first_seed".to_string()),
        (CoreNodeId(11), "second_seed".to_string()),
        (CoreNodeId(20), "proxy_seed".to_string()),
        (CoreNodeId(30), "convergence".to_string()),
    ]);
    let edge_permutations = all_permutations(&edges);
    let seed_permutations = all_permutations(&seeds);
    assert_eq!(edge_permutations.len(), 120);
    assert_eq!(seed_permutations.len(), 6);

    let mut expected = None;
    for edge_order in edge_permutations {
        for seed_order in &seed_permutations {
            let seed_evidence = seed_order.iter().cloned().collect::<BTreeMap<_, _>>();
            let walk = affected_reverse_walk(2, &edge_order, seed_evidence, &labels);
            if let Some(expected) = expected.as_ref() {
                assert_eq!(&walk, expected);
            } else {
                expected = Some(walk);
            }
        }
    }

    let walk = expected.expect("deterministic reverse walk");
    assert_eq!(walk.visited_edge_count, 5);
    let convergence = &walk.evidence[&CoreNodeId(30)];
    assert_eq!(convergence.confidence_floor.label(), "certain");
    assert!(!convergence.previous_identity_proxy);
    assert_eq!(
        convergence.path_tie_key,
        vec![
            AffectedPathTieStep {
                edge_id: i64::MIN,
                source_node_id: CoreNodeId(10),
                target_node_id: CoreNodeId(10),
            },
            AffectedPathTieStep {
                edge_id: 100,
                source_node_id: CoreNodeId(30),
                target_node_id: CoreNodeId(10),
            },
        ]
    );
    assert_eq!(
        walk.evidence[&CoreNodeId(40)].confidence_floor.label(),
        "uncertain",
        "the weakest edge floor must survive the complete path"
    );
    assert_eq!(
        walk.evidence[&CoreNodeId(50)].confidence_floor.label(),
        "bounded",
        "a previous-identity proxy must remain bounded across traversal"
    );
}

#[test]
fn affected_route_confidence_is_the_weaker_graph_or_metadata_floor() {
    let probable = AffectedConfidenceFloor::from_label("probable");
    for structured_label in ["file_convention", "decorator", "annotation", "attribute"] {
        assert_eq!(
            AffectedConfidenceFloor::from_label(structured_label).strength,
            probable.strength,
            "{structured_label} must retain the probable structured-evidence tier"
        );
    }
    assert!(probable.strength > AffectedConfidenceFloor::from_label("graph").strength);
    assert!(
        AffectedConfidenceFloor::from_label("graph").strength
            > AffectedConfidenceFloor::from_label("heuristic").strength
    );

    let graph = AffectedGraphEvidence::seed(
        CoreNodeId(1),
        "graph route evidence",
        AffectedConfidenceFloor::from_label("certain"),
        false,
    );
    let nested_metadata_label = "heuristic".to_string();
    assert_eq!(
        affected_route_confidence(&graph, Some(&nested_metadata_label)),
        "heuristic"
    );
    assert_eq!(nested_metadata_label, "heuristic");

    let proxy = AffectedGraphEvidence::seed(
        CoreNodeId(2),
        "proxy route evidence",
        AffectedConfidenceFloor::from_label("schema"),
        true,
    );
    assert_eq!(affected_route_confidence(&proxy, Some("schema")), "bounded");
    assert_eq!(affected_route_confidence(&graph, None), "graph");
}

#[test]
fn route_handler_comparator_is_total_across_every_candidate_permutation() {
    let candidate = |edge_id: i64,
                     confidence: Option<f32>,
                     certainty: ResolutionCertainty,
                     semantic: &str,
                     start_line: u32,
                     target_id: i64| RouteHandlerCandidate {
        edge: Edge {
            id: EdgeId(edge_id),
            source: CoreNodeId(900),
            target: CoreNodeId(target_id),
            kind: EdgeKind::CALL,
            confidence,
            certainty: Some(certainty),
            ..Default::default()
        },
        target: Node {
            id: CoreNodeId(target_id),
            kind: NodeKind::FUNCTION,
            serialized_name: semantic.to_string(),
            qualified_name: Some(semantic.to_string()),
            canonical_id: Some(semantic.to_string()),
            file_node_id: Some(CoreNodeId(800)),
            start_line: Some(start_line),
            start_col: Some(1),
            end_line: Some(start_line),
            end_col: Some(2),
        },
    };
    let candidates = vec![
        candidate(1, Some(0.9), ResolutionCertainty::Uncertain, "zeta", 9, 9),
        candidate(2, Some(0.8), ResolutionCertainty::Certain, "alpha", 1, 1),
        candidate(3, Some(0.8), ResolutionCertainty::Certain, "alpha", 1, 1),
        candidate(4, Some(0.8), ResolutionCertainty::Certain, "alpha", 1, 2),
        candidate(5, Some(0.8), ResolutionCertainty::Certain, "alpha", 2, 3),
        candidate(
            6,
            Some(0.8),
            ResolutionCertainty::Probable,
            "aardvark",
            1,
            4,
        ),
        candidate(7, Some(0.8), ResolutionCertainty::Certain, "beta", 1, 5),
    ];
    let permutations = all_permutations(&candidates);
    assert_eq!(permutations.len(), 5_040);
    for mut permutation in permutations {
        permutation.sort_by(compare_route_handler_candidates);
        assert_eq!(
            permutation
                .iter()
                .map(|candidate| candidate.edge.id.0)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 7, 6]
        );
    }
    for non_finite in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            compare_optional_confidence_desc(Some(non_finite), None),
            Ordering::Equal,
            "non-finite confidence must rank as absent"
        );
    }
    assert_eq!(
        compare_optional_confidence_desc(Some(0.0), None),
        Ordering::Less,
        "every finite confidence must rank ahead of absent quality"
    );

    let hostile = vec![
        candidate(11, Some(0.9), ResolutionCertainty::Certain, "same", 1, 100),
        candidate(12, Some(0.4), ResolutionCertainty::Certain, "same", 1, 100),
        candidate(13, None, ResolutionCertainty::Certain, "same", 1, 100),
        candidate(
            14,
            Some(f32::NAN),
            ResolutionCertainty::Certain,
            "same",
            1,
            100,
        ),
        candidate(
            15,
            Some(f32::INFINITY),
            ResolutionCertainty::Certain,
            "same",
            1,
            100,
        ),
        candidate(
            16,
            Some(f32::NEG_INFINITY),
            ResolutionCertainty::Certain,
            "same",
            1,
            100,
        ),
    ];
    let permutations = all_permutations(&hostile);
    assert_eq!(permutations.len(), 720);
    for mut permutation in permutations {
        permutation.sort_by(compare_route_handler_candidates);
        assert_eq!(
            permutation
                .iter()
                .map(|candidate| candidate.edge.id.0)
                .collect::<Vec<_>>(),
            vec![11, 12, 13, 14, 15, 16]
        );
    }
    for left in &hostile {
        assert_eq!(
            compare_route_handler_candidates(left, left),
            Ordering::Equal
        );
        for right in &hostile {
            assert_eq!(
                compare_route_handler_candidates(left, right),
                compare_route_handler_candidates(right, left).reverse()
            );
            for third in &hostile {
                let left_to_right = compare_route_handler_candidates(left, right);
                let right_to_third = compare_route_handler_candidates(right, third);
                if left_to_right != Ordering::Greater && right_to_third != Ordering::Greater {
                    assert_ne!(
                        compare_route_handler_candidates(left, third),
                        Ordering::Greater
                    );
                }
            }
        }
    }
}

#[test]
fn affected_graph_cycle_terminates_and_result_caps_are_enforced() {
    let project = tempdir().expect("project");
    let source_dir = project.path().join("src");
    fs::create_dir_all(&source_dir).expect("create source directory");
    let source_path = source_dir.join("lib.rs");
    fs::write(&source_path, "pub fn seed() {}\n").expect("write source");
    let modification_time = fs::metadata(&source_path)
        .expect("source metadata")
        .modified()
        .expect("source mtime")
        .duration_since(UNIX_EPOCH)
        .expect("mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;

    {
        let mut storage = Storage::open(project.path().join("codestory.db")).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path.clone(),
                language: "rust".to_string(),
                modification_time,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert source file");
        let mut nodes = vec![
            Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: source_path.to_string_lossy().to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(2),
                kind: NodeKind::FUNCTION,
                serialized_name: "seed".to_string(),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(1),
                ..Default::default()
            },
        ];
        let mut edges = Vec::new();
        for index in 0..220_i64 {
            let node_id = CoreNodeId(1_000 + index);
            nodes.push(Node {
                id: node_id,
                kind: NodeKind::FUNCTION,
                serialized_name: format!("route_{index}"),
                canonical_id: Some(format!("openapi:endpoint:GET /route/{index}")),
                start_line: Some(1),
                ..Default::default()
            });
            edges.push(Edge {
                id: EdgeId(2_000 + index),
                source: node_id,
                target: CoreNodeId(2),
                kind: EdgeKind::CALL,
                ..Default::default()
            });
        }
        edges.push(Edge {
            id: EdgeId(9_999),
            source: CoreNodeId(2),
            target: CoreNodeId(1_000),
            kind: EdgeKind::CALL,
            ..Default::default()
        });
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage.insert_edges_batch(&edges).expect("insert edges");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: project.path().to_string_lossy().to_string(),
        })
        .expect("open project");
    let result = controller
        .affected_analysis(AffectedAnalysisRequest {
            input: AffectedAnalysisInput::Paths(vec!["src/lib.rs".to_string()]),
            depth: Some(8),
            filter: None,
        })
        .expect("affected analysis");

    assert_eq!(result.impacted_symbols.len(), 200);
    assert_eq!(result.impacted_routes.len(), 100);
    assert!(result.completeness.truncated);
    assert!(!result.completeness.complete);
    assert_eq!(result.bounds.impacted_symbol_limit, 200);
    assert_eq!(result.bounds.impacted_route_limit, 100);
    assert!(
        result.bounds.visited_node_count <= 222,
        "cycle should not revisit nodes without bound: {:?}",
        result.bounds
    );
    assert_eq!(
        result.completeness.truncation_reasons,
        vec![
            "impacted_symbols retained 200 of 221 results".to_string(),
            "impacted_routes retained 100 of 220 results".to_string(),
        ]
    );
}

#[test]
fn indexable_source_path_tracks_indexer_structural_and_template_surfaces() {
    for relative_path in [
        "src/lib.rs",
        "src/main.go",
        "src/App.vue",
        "src/App.svelte",
        "src/pages/index.astro",
        "public/index.html",
        "public/site.css",
        "db/schema.sql",
        "docs/guide.mdx",
        "config/service.yaml",
        "config/service.toml",
        "config/service.json",
        "scripts/setup.zsh",
        "scripts/build.ps1",
    ] {
        assert!(
            indexable_source_path(Path::new(relative_path)),
            "runtime freshness should count indexer-indexable path: {relative_path}"
        );
    }
}

#[test]
fn indexable_source_path_keeps_non_code_data_outside_freshness_gate() {
    assert!(
        !indexable_source_path(Path::new("target/run-output.log")),
        "runtime freshness should not count unsupported output artifacts"
    );
    for excluded in [
        "vendor/config.json",
        "generated/docs.md",
        "config/package-lock.json",
        "skills-lock.json",
        "secrets/deploy.ps1",
        "web/app.min.json",
    ] {
        assert!(
            !indexable_source_path(Path::new(excluded)),
            "runtime freshness should honor structural exclusion: {excluded}"
        );
    }
}

#[test]
fn dedicated_openapi_coverage_requires_authenticated_file_owned_projection_evidence() {
    let project = tempdir().expect("project");
    let mut storage = Storage::new_in_memory().expect("storage");
    let sources = [
        (
            "openapi.json",
            "{\"openapi\":\"3.1.0\",\"paths\":{\"/json-ready\":{\"get\":{}}}}\n",
        ),
        (
            "openapi.yaml",
            "openapi: 3.1.0\npaths:\n  /yaml-ready:\n    get:\n      responses: {}\n",
        ),
        ("config.json", "{\"enabled\":true}\n"),
    ];
    let files_to_index = sources
        .iter()
        .map(|(relative, source)| {
            let path = project.path().join(relative);
            fs::write(&path, source).expect("write projected source");
            path
        })
        .collect::<Vec<_>>();
    V2WorkspaceIndexer::new(project.path().to_path_buf())
        .run(
            &mut storage,
            &RefreshExecutionPlan {
                mode: RefreshMode::FullRefresh,
                files_to_index,
                files_to_remove: Vec::new(),
                existing_file_ids: HashMap::new(),
            },
            &EventBus::new(),
            None,
        )
        .expect("index real OpenAPI and generic JSON projections");
    let real_diagnostics = stored_file_coverage_diagnostics(project.path(), &storage)
        .expect("verify real projections");
    assert!(
        real_diagnostics.is_empty(),
        "real projections must authenticate coverage: {real_diagnostics:#?}"
    );

    for (id, relative, language) in [
        (9_000_001, "metadata-only.json", "openapi"),
        (9_000_002, "forged-openapi.json", "openapi"),
        (9_000_003, "wrong-language.json", "json"),
    ] {
        let path = project.path().join(relative);
        fs::write(&path, "{}\n").expect("write structural source");
        let file = FileInfo {
            id,
            path,
            language: language.to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        };
        storage.insert_file(&file).expect("insert indexed file");
        storage
            .update_file_metadata(&file, Some(&format!("{id:064x}")))
            .expect("persist verified source identity");
    }
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(9_000_002),
                kind: NodeKind::FILE,
                serialized_name: project
                    .path()
                    .join("forged-openapi.json")
                    .to_string_lossy()
                    .to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(9_100_002),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /forged".to_string(),
                canonical_id: Some("openapi:endpoint:GET /forged".to_string()),
                file_node_id: Some(CoreNodeId(9_000_002)),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(9_000_003),
                kind: NodeKind::FILE,
                serialized_name: project
                    .path()
                    .join("wrong-language.json")
                    .to_string_lossy()
                    .to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(9_100_003),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /wrong-language".to_string(),
                canonical_id: Some("openapi:endpoint:GET /wrong-language".to_string()),
                file_node_id: Some(CoreNodeId(9_000_003)),
                ..Default::default()
            },
        ])
        .expect("insert forged endpoint nodes");
    storage
        .insert_edges_batch(&[Edge {
            id: EdgeId(9_200_003),
            source: CoreNodeId(9_000_003),
            target: CoreNodeId(9_100_003),
            kind: EdgeKind::MEMBER,
            file_node_id: Some(CoreNodeId(9_000_003)),
            ..Default::default()
        }])
        .expect("insert wrong-language member edge");
    storage
        .insert_occurrences_batch(&[Occurrence {
            element_id: 9_100_003,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: CoreNodeId(9_000_003),
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 2,
            },
        }])
        .expect("insert wrong-language definition occurrence");
    assert!(
        storage
            .has_file_owned_openapi_endpoint_projection(9_000_003)
            .expect("verify wrong-language graph evidence"),
        "runtime language check must reject otherwise authenticated OpenAPI graph evidence"
    );

    let diagnostics = stored_file_coverage_diagnostics(project.path(), &storage)
        .expect("load stored file coverage");
    assert_eq!(diagnostics.len(), 3);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.path.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from([
            "metadata-only.json",
            "forged-openapi.json",
            "wrong-language.json",
        ])
    );
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.reason
        == FileCoverageReason::CollectorFailure
        && diagnostic.verified_source
        && !diagnostic.projection_available));
}

#[test]
fn incremental_openapi_structural_transitions_replace_file_owned_projection_atomically() {
    fn endpoint_ids(storage: &Storage) -> HashSet<String> {
        storage
            .get_nodes()
            .expect("load endpoint nodes")
            .into_iter()
            .filter_map(|node| node.canonical_id)
            .filter(|canonical_id| canonical_id.starts_with("openapi:endpoint:"))
            .collect()
    }

    let workspace = tempdir().expect("workspace");
    let schema_path = workspace.path().join("schema.json");
    fs::write(
        &schema_path,
        "{\"openapi\":\"3.1.0\",\"paths\":{\"/old\":{\"get\":{}}}}\n",
    )
    .expect("write baseline OpenAPI source");
    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open OpenAPI project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline OpenAPI projection");
    let baseline = Storage::open(&storage_path)
        .expect("open baseline")
        .get_complete_index_publication()
        .expect("read baseline publication")
        .expect("baseline publication");
    assert_eq!(
        endpoint_ids(&Storage::open(&storage_path).expect("reopen baseline")),
        HashSet::from(["openapi:endpoint:GET /old".to_string()])
    );

    fs::write(
        &schema_path,
        "{\"openapi\":\"3.1.0\",\"paths\":{\"/renamed\":{\"post\":{}}}}\n",
    )
    .expect("write renamed OpenAPI endpoint");
    arm_publication_test_fault(
        PublicationTestBoundary::MarkerCompletion,
        PublicationTestAction::Fail,
    );
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect_err("injected staged transition failure must reject publication");
    assert_eq!(error.code, "internal");
    let preserved = Storage::open(&storage_path).expect("open preserved baseline");
    assert_eq!(
        preserved
            .get_complete_index_publication()
            .expect("read preserved publication"),
        Some(baseline)
    );
    assert_eq!(
        endpoint_ids(&preserved),
        HashSet::from(["openapi:endpoint:GET /old".to_string()])
    );
    drop(preserved);
    assert_no_staged_publication_artifacts(&storage_path);

    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("publish renamed OpenAPI endpoint");
    let renamed = Storage::open(&storage_path).expect("open renamed projection");
    assert_eq!(
        endpoint_ids(&renamed),
        HashSet::from(["openapi:endpoint:POST /renamed".to_string()])
    );
    let file = renamed
        .get_file_by_path(&schema_path)
        .expect("read renamed file")
        .expect("renamed file");
    assert_eq!(file.language, "openapi");
    assert!(
        renamed
            .has_file_owned_openapi_endpoint_projection(file.id)
            .expect("authenticate renamed endpoint")
    );
    drop(renamed);

    fs::write(&schema_path, "{\"enabled\":true}\n").expect("write generic JSON source");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("publish OpenAPI to generic transition");
    let generic = Storage::open(&storage_path).expect("open generic projection");
    assert!(endpoint_ids(&generic).is_empty());
    let file = generic
        .get_file_by_path(&schema_path)
        .expect("read generic file")
        .expect("generic file");
    assert_eq!(file.language, "json");
    assert!(
        !generic
            .has_file_owned_openapi_endpoint_projection(file.id)
            .expect("reject removed OpenAPI endpoint")
    );
    assert!(
        generic
            .get_structural_text_projection_file_ids()
            .expect("read generic structural projections")
            .contains(&file.id)
    );
    drop(generic);

    fs::write(
        &schema_path,
        "{\"openapi\":\"3.1.0\",\"paths\":{\"/restored\":{\"get\":{}}}}\n",
    )
    .expect("write restored OpenAPI source");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("publish generic to OpenAPI transition");
    let restored = Storage::open(&storage_path).expect("open restored projection");
    assert_eq!(
        endpoint_ids(&restored),
        HashSet::from(["openapi:endpoint:GET /restored".to_string()])
    );
    let file = restored
        .get_file_by_path(&schema_path)
        .expect("read restored file")
        .expect("restored file");
    assert_eq!(file.language, "openapi");
    assert!(
        restored
            .has_file_owned_openapi_endpoint_projection(file.id)
            .expect("authenticate restored endpoint")
    );
    assert!(
        !restored
            .get_structural_text_projection_file_ids()
            .expect("read restored structural projections")
            .contains(&file.id)
    );
    assert!(
        stored_file_coverage_diagnostics(workspace.path(), &restored)
            .expect("verify restored coverage")
            .is_empty()
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn affected_absolute_excluded_structural_path_is_valid_but_uncovered() {
    let project = tempdir().expect("project");
    let excluded = project.path().join("vendor/config.json");
    fs::create_dir_all(excluded.parent().expect("excluded parent"))
        .expect("create excluded parent");
    fs::write(&excluded, "{\"ignored\":true}\n").expect("write excluded source");
    assert!(!indexable_source_path_in_workspace(
        project.path(),
        &excluded
    ));
    let resolved = AffectedResolvedInput {
        current: excluded.clone(),
        previous: None,
    };
    let freshness =
        affected_test_freshness(project.path(), IndexFreshnessStatusDto::Fresh, Vec::new());
    let identity =
        codestory_workspace::workspace_path_identity(&excluded).expect("excluded identity");
    let (classification, reason, _) = classify_unmatched_affected_input(
        Some(project.path()),
        &affected_test_record(AffectedChangeKindDto::Modified, "vendor/config.json"),
        &resolved,
        &freshness,
        Some(&identity),
        None,
        None,
    );
    assert_eq!(
        classification,
        AffectedInputClassificationDto::ValidUncovered
    );
    assert!(reason.contains("outside current graph/index coverage"));
}

fn insert_current_indexed_file(storage: &Storage, id: i64, path: &Path) {
    let modification_time = fs::metadata(path)
        .expect("source metadata")
        .modified()
        .expect("source mtime")
        .duration_since(UNIX_EPOCH)
        .expect("mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;
    storage
        .insert_file(&FileInfo {
            id,
            path: path.to_path_buf(),
            language: "rust".into(),
            modification_time,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        })
        .expect("insert current indexed file");
}

#[test]
fn newly_discovered_hardlink_alias_remains_new_for_freshness() {
    let project = tempdir().expect("project");
    let indexed = project.path().join("lib.rs");
    let alias = project.path().join("alias.rs");
    fs::write(&indexed, "pub fn indexed() {}\n").expect("write indexed source");
    fs::hard_link(&indexed, &alias).expect("create source hardlink alias");
    let workspace = WorkspaceManifest::open(project.path().to_path_buf()).expect("workspace");
    let storage = Storage::new_in_memory().expect("storage");
    insert_current_indexed_file(&storage, 1, &indexed);

    let freshness = index_freshness_from_storage(project.path(), &workspace, &storage);

    assert_eq!(freshness.status, IndexFreshnessStatusDto::Stale);
    assert_eq!(freshness.changed_file_count, 0);
    assert_eq!(freshness.new_file_count, 1);
    assert_eq!(freshness.removed_file_count, 0);
    assert!(freshness.samples.iter().any(|sample| {
        sample.kind == IndexFreshnessChangeKindDto::New && sample.path == "alias.rs"
    }));
}

#[test]
fn unsupported_extension_hardlink_alias_stays_outside_freshness() {
    let project = tempdir().expect("project");
    let indexed = project.path().join("lib.rs");
    let alias = project.path().join("source-alias.unsupported-binary");
    fs::write(&indexed, "pub fn indexed() {}\n").expect("write indexed source");
    fs::hard_link(&indexed, &alias).expect("create unsupported hardlink alias");
    assert!(!indexable_source_path(&alias));
    let workspace = WorkspaceManifest::open(project.path().to_path_buf()).expect("workspace");
    let storage = Storage::new_in_memory().expect("storage");
    insert_current_indexed_file(&storage, 1, &indexed);

    let freshness = index_freshness_from_storage(project.path(), &workspace, &storage);

    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(freshness.changed_file_count, 0);
    assert_eq!(freshness.new_file_count, 0);
    assert_eq!(freshness.removed_file_count, 0);
    assert!(freshness.samples.is_empty());
}

#[test]
fn incomplete_run_fence_wins_before_indexed_inventory() {
    let project = tempdir().expect("project");
    let workspace = WorkspaceManifest::open(project.path().to_path_buf()).expect("workspace");
    let storage = Storage::new_in_memory().expect("storage");
    storage
        .begin_incremental_run()
        .expect("install incomplete-run fence");
    storage
        .get_connection()
        .pragma_update(None, "foreign_keys", "OFF")
        .expect("disable fixture foreign keys");
    storage
        .get_connection()
        .execute("DROP TABLE file", [])
        .expect("remove indexed inventory");
    storage
        .get_connection()
        .pragma_update(None, "foreign_keys", "ON")
        .expect("restore fixture foreign keys");

    let freshness = index_freshness_from_storage(project.path(), &workspace, &storage);

    assert_eq!(freshness.status, IndexFreshnessStatusDto::Stale);
    assert_eq!(
        freshness.reason.as_deref(),
        Some("previous_incremental_run_incomplete_full_refresh_required")
    );
    assert_eq!(freshness.changed_file_count, 0);
    assert_eq!(freshness.new_file_count, 0);
    assert_eq!(freshness.removed_file_count, 0);
    assert_eq!(freshness.checked_file_count, 0);
    assert_eq!(freshness.indexed_file_count, 0);
    assert!(freshness.samples.is_empty());
}

#[test]
fn owned_freshness_observation_pins_fence_and_inventory_across_a_concurrent_writer() {
    let project = tempdir().expect("project");
    let source_path = project.path().join("lib.rs");
    fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
    let modification_time = fs::metadata(&source_path)
        .expect("source metadata")
        .modified()
        .expect("source mtime")
        .duration_since(UNIX_EPOCH)
        .expect("mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let database = tempdir().expect("database directory");
    let storage_path = database.path().join("codestory.db");
    let keeper = Storage::open(&storage_path).expect("open fixture storage");
    keeper
        .insert_file(&FileInfo {
            id: 1,
            path: source_path,
            language: "rust".into(),
            modification_time,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        })
        .expect("insert coherent file projection");
    assert!(
        storage_path.with_extension("db-wal").is_file(),
        "fixture must retain WAL state"
    );
    assert!(
        storage_path.with_extension("db-shm").is_file(),
        "fixture must retain its WAL index"
    );
    let workspace = WorkspaceManifest::open(project.path().to_path_buf()).expect("workspace");
    let observed = Storage::open_freshness_observational(&storage_path)
        .expect("open owned freshness snapshot");

    let (release_writer, writer_released) = std::sync::mpsc::channel();
    let (writer_done, await_writer) = std::sync::mpsc::channel();
    let writer_storage_path = storage_path.clone();
    let projected_after_fence = project.path().join("removed-after-fence.rs");
    let writer = std::thread::spawn(move || {
        writer_released.recv().expect("release concurrent writer");
        let storage = Storage::open(&writer_storage_path).expect("open concurrent writer");
        storage
            .begin_incremental_run()
            .expect("install concurrent incomplete fence");
        storage
            .insert_file(&FileInfo {
                id: 2,
                path: projected_after_fence,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("mutate projection after the fence read");
        writer_done.send(()).expect("signal writer completion");
    });
    arm_after_index_freshness_fence_test_hook(move || {
        release_writer.send(()).expect("start concurrent writer");
        await_writer.recv().expect("wait for concurrent writer");
    });

    let before = index_freshness_from_storage(project.path(), &workspace, &observed);
    writer.join().expect("join concurrent writer");
    assert_eq!(before.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(before.changed_file_count, 0);
    assert_eq!(before.new_file_count, 0);
    assert_eq!(before.removed_file_count, 0);
    assert_eq!(before.indexed_file_count, 1);
    assert!(before.samples.is_empty());
    drop(observed);

    let fenced = Storage::open_freshness_observational(&storage_path)
        .expect("open post-writer freshness snapshot");
    let after = index_freshness_from_storage(project.path(), &workspace, &fenced);
    assert_eq!(after.status, IndexFreshnessStatusDto::Stale);
    assert_eq!(
        after.reason.as_deref(),
        Some("previous_incremental_run_incomplete_full_refresh_required")
    );
    assert_eq!(after.changed_file_count, 0);
    assert_eq!(after.new_file_count, 0);
    assert_eq!(after.removed_file_count, 0);
    assert_eq!(after.indexed_file_count, 0);
    assert!(after.samples.is_empty());
    drop(fenced);
    drop(keeper);
}

#[test]
fn parser_partial_freshness_distinguishes_file_level_errors() {
    let project = tempdir().expect("project");
    let source_path = project.path().join("lib.rs");
    fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
    let modification_time = fs::metadata(&source_path)
        .expect("metadata")
        .modified()
        .expect("mtime")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("mtime since epoch")
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let workspace = WorkspaceManifest::open(project.path().to_path_buf()).expect("workspace");
    let storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_file(&FileInfo {
            id: 1,
            path: source_path,
            language: "rust".into(),
            modification_time,
            indexed: true,
            complete: false,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        })
        .expect("insert parser-partial file");

    let freshness = index_freshness_from_storage(project.path(), &workspace, &storage);
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(freshness.changed_file_count, 0);

    storage
        .insert_error(&codestory_contracts::graph::ErrorInfo {
            message: "read failed".into(),
            file_id: Some(codestory_contracts::graph::NodeId(1)),
            line: None,
            column: None,
            is_fatal: true,
            index_step: codestory_contracts::graph::IndexStep::Indexing,
            coverage_reason: Some(FileCoverageReason::Unreadable),
        })
        .expect("file error");
    let retry = index_freshness_from_storage(project.path(), &workspace, &storage);
    assert_eq!(retry.status, IndexFreshnessStatusDto::Stale);
    assert_eq!(retry.changed_file_count, 1);
}

#[test]
fn incomplete_workspace_inventory_is_not_reported_as_removed_files() {
    let project = tempdir().expect("project");
    let source_path = project.path().join("stored.rs");
    let manifest = serde_json::json!({
        "name": "repo",
        "version": 1,
        "source_groups": [{
            "id": Uuid::new_v4(),
            "language": "Rust",
            "standard": "Default",
            "source_paths": ["unreadable"],
            "exclude_patterns": [],
            "include_paths": [],
            "defines": {},
            "language_specific": "Other"
        }]
    });
    fs::write(
        project.path().join("codestory_project.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    let workspace = WorkspaceManifest::open(project.path().to_path_buf()).expect("workspace");
    let storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_file(&FileInfo {
            id: 1,
            path: source_path,
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        })
        .expect("insert stored file");

    let freshness = index_freshness_from_storage(project.path(), &workspace, &storage);
    assert_eq!(freshness.status, IndexFreshnessStatusDto::NotChecked);
    assert_eq!(freshness.removed_file_count, 0);
    assert!(
        freshness
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Unreadable")),
        "unexpected freshness reason: {:?}",
        freshness.reason
    );
}
