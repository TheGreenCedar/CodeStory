use super::*;
use crate::search::lexical::exact_symbol_merged_lexical_queries;
use codestory_contracts::graph::{
    Edge, EdgeId, EdgeKind, Node, NodeId as CoreNodeId, NodeKind, Occurrence, OccurrenceKind,
    ResolutionCertainty, SourceLocation,
};
use crossbeam_channel::unbounded;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard as StdMutexGuard;
use tempfile::tempdir;

#[path = "tests/activation_coverage_tests.rs"]
mod activation_coverage_tests;

#[test]
fn runtime_facade_remains_inside_the_default_source_index_cap() {
    let facade = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let bytes = fs::metadata(&facade)
        .expect("read runtime facade metadata")
        .len();

    assert!(
        bytes <= DEFAULT_SOURCE_FILE_BYTE_CAP,
        "{} is {bytes} bytes, above the {DEFAULT_SOURCE_FILE_BYTE_CAP}-byte source-index cap",
        facade.display()
    );
}

fn default_source_policy_identity() -> SourcePolicyExclusionPolicyIdentity<'static> {
    SourcePolicyExclusionPolicyIdentity::new(
        OVERSIZED_SOURCE_POLICY_VERSION,
        DEFAULT_SOURCE_FILE_BYTE_CAP,
        codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
    )
}

fn legacy_source_policy_identity() -> SourcePolicyExclusionPolicyIdentity<'static> {
    SourcePolicyExclusionPolicyIdentity::new(
        LEGACY_OVERSIZED_SOURCE_POLICY_VERSION,
        DEFAULT_SOURCE_FILE_BYTE_CAP,
        codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
    )
}

fn legacy_source_policy_exclusion_digest_for_test(
    records: &[SourcePolicyExclusionRecord],
) -> String {
    fn hash_part(hasher: &mut Sha256, value: &[u8]) {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value);
    }

    let mut hasher = Sha256::new();
    hasher.update(b"codestory-source-policy-exclusion-publication-v1\0");
    for record in records {
        for value in [
            record.normalized_path.as_bytes(),
            record.project_id.as_bytes(),
            record.workspace_id.as_bytes(),
            record.content_hash.as_bytes(),
            record.policy_version.as_bytes(),
            record.core_generation_id.as_bytes(),
            record.core_run_id.as_bytes(),
        ] {
            hash_part(&mut hasher, value);
        }
        hash_part(&mut hasher, &record.observed_size.to_le_bytes());
        hash_part(&mut hasher, &record.byte_cap.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn semantic_projection_source_policy_bridge_is_directional_and_cap_exact() {
    let current = SourceIndexPolicy::default();
    assert_eq!(
        semantic_projection_source_policy_compatibility(
            default_source_policy_identity(),
            &current,
            30,
            false,
        ),
        Some(SemanticProjectionSourcePolicyCompatibility::Exact)
    );
    assert_eq!(
        semantic_projection_source_policy_compatibility(
            legacy_source_policy_identity(),
            &current,
            LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION,
            true,
        ),
        Some(SemanticProjectionSourcePolicyCompatibility::LegacyPredecessor)
    );
    for recorded in [
        SourcePolicyExclusionPolicyIdentity::new(
            "unknown-source-policy",
            current.byte_cap,
            current.structural_unit_cap,
        ),
        SourcePolicyExclusionPolicyIdentity::new(
            LEGACY_OVERSIZED_SOURCE_POLICY_VERSION,
            current.byte_cap + 1,
            current.structural_unit_cap,
        ),
        SourcePolicyExclusionPolicyIdentity::new(
            LEGACY_OVERSIZED_SOURCE_POLICY_VERSION,
            current.byte_cap,
            current.structural_unit_cap + 1,
        ),
    ] {
        assert_eq!(
            semantic_projection_source_policy_compatibility(
                recorded,
                &current,
                LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION,
                true,
            ),
            None
        );
    }
    assert_eq!(
        semantic_projection_source_policy_compatibility(
            legacy_source_policy_identity(),
            &current,
            30,
            true,
        ),
        None
    );
    assert_eq!(
        semantic_projection_source_policy_compatibility(
            legacy_source_policy_identity(),
            &current,
            LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION,
            false,
        ),
        None
    );
    let legacy_runtime = SourceIndexPolicy {
        policy_version: LEGACY_OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
        byte_cap: current.byte_cap,
        structural_unit_cap: current.structural_unit_cap,
    };
    assert_eq!(
        semantic_projection_source_policy_compatibility(
            default_source_policy_identity(),
            &legacy_runtime,
            LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION,
            true,
        ),
        None
    );
}

fn test_retrieval_manifest(
    project_id: &str,
    symbol_doc_count: i64,
    dense_projection_count: i64,
) -> RetrievalIndexManifest {
    RetrievalIndexManifest {
        project_id: project_id.to_string(),
        lexical_version: "retained-v1".to_string(),
        semantic_generation: "retained-v1".to_string(),
        scip_revision: None,
        built_at_epoch_ms: 1,
        disk_bytes: Some(1),
        degraded_modes_json: "[]".to_string(),
        embedding_backend: Some("retained-v1".to_string()),
        embedding_dim: Some(1),
        sidecar_schema_version: Some(1),
        sidecar_input_hash: Some("1".repeat(64)),
        sidecar_generation: Some("retained-v1".to_string()),
        projection_count: Some(1),
        symbol_doc_count: Some(symbol_doc_count),
        dense_projection_count: Some(dense_projection_count),
        semantic_policy_version: Some(SEMANTIC_POLICY_VERSION.to_string()),
        graph_artifact_hash: Some("2".repeat(64)),
        dense_reason_counts_json: Some("{}".to_string()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    }
}

#[test]
fn full_refresh_wall_residual_uses_raw_durations_before_millis_conversion() {
    let wall = FullRefreshWallDurations {
        live_inspection: Duration::from_micros(750),
        source_discovery: Duration::from_micros(250),
        ..FullRefreshWallDurations::default()
    }
    .finish(Duration::from_micros(1_500));

    assert_eq!(wall.core_refresh_ms, 1);
    assert_eq!(wall.live_inspection_ms, 0);
    assert_eq!(wall.source_discovery_ms, 0);
    assert_eq!(wall.unattributed_ms, 0);
}

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

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
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

fn assert_mandatory_retrieval_unavailable(error: &ApiError) {
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

struct HybridTestEnv {
    guards: Vec<EnvGuard>,
    _lock: StdMutexGuard<'static, ()>,
}

impl HybridTestEnv {
    fn push(&mut self, guard: EnvGuard) {
        self.guards.push(guard);
    }

    fn pop(&mut self) {
        self.guards.pop();
    }
}

fn hybrid_test_env() -> HybridTestEnv {
    let lock = process_env_test_lock();
    HybridTestEnv {
        guards: vec![
            EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "true"),
            EnvGuard::remove(SEMANTIC_DOC_SCOPE_ENV),
            EnvGuard::remove(SEMANTIC_DOC_ALIAS_MODE_ENV),
            EnvGuard::remove(SEMANTIC_DOC_MAX_TOKENS_ENV),
            EnvGuard::remove(SEMANTIC_STREAM_PENDING_DOCS_ENV),
            EnvGuard::remove(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV),
        ],
        _lock: lock,
    }
}

#[test]
fn graph_edge_dto_defaults_structural_member_certainty() {
    let flags = AppGraphFeatureFlags {
        include_edge_certainty: true,
        include_callsite_identity: true,
        include_candidate_targets: true,
    };

    let member = graph_edge_dto(
        Edge {
            id: EdgeId(1),
            source: CoreNodeId(10),
            target: CoreNodeId(20),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
        flags,
    );
    let unresolved_call = graph_edge_dto(
        Edge {
            id: EdgeId(2),
            source: CoreNodeId(10),
            target: CoreNodeId(30),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        flags,
    );
    let explicit_probable = graph_edge_dto(
        Edge {
            id: EdgeId(3),
            source: CoreNodeId(10),
            target: CoreNodeId(40),
            kind: EdgeKind::MEMBER,
            certainty: Some(ResolutionCertainty::Probable),
            ..Default::default()
        },
        flags,
    );

    assert_eq!(member.certainty.as_deref(), Some("certain"));
    assert_eq!(unresolved_call.certainty, None);
    assert_eq!(explicit_probable.certainty.as_deref(), Some("probable"));
}

#[test]
fn parse_search_intent_query_extracts_supported_filters() {
    let parsed = parse_search_intent_query(
        "kind:function name:`listUsers` path:src/routes.ts lang:typescript",
    );

    assert_eq!(parsed.effective_query, "listUsers");
    assert_eq!(
        parsed.filters,
        vec![
            SearchIntentFilter::Kind("function".to_string()),
            SearchIntentFilter::Name("listUsers".to_string()),
            SearchIntentFilter::Path("src/routes.ts".to_string()),
            SearchIntentFilter::Language("typescript".to_string()),
        ]
    );

    let unknown_prefix = parse_search_intent_query("owner:web /api/users");
    assert_eq!(unknown_prefix.effective_query, "owner:web /api/users");
    assert!(unknown_prefix.filters.is_empty());
}

#[test]
fn search_intent_filters_hits_by_kind_path_name_and_language() {
    fn hit(
        id: &str,
        display_name: &str,
        kind: codestory_contracts::api::NodeKind,
        file_path: &str,
    ) -> SearchHit {
        SearchHit {
            node_id: codestory_contracts::api::NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score: 1.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }
    }

    let mut hits = vec![
        hit(
            "a",
            "listUsers",
            codestory_contracts::api::NodeKind::FUNCTION,
            "src/routes.ts",
        ),
        hit(
            "b",
            "Users",
            codestory_contracts::api::NodeKind::STRUCT,
            "src/routes.ts",
        ),
        hit(
            "c",
            "listUsers",
            codestory_contracts::api::NodeKind::FUNCTION,
            "src/routes.rs",
        ),
    ];

    apply_search_intent_filters(
        &mut hits,
        &[
            SearchIntentFilter::Kind("function".to_string()),
            SearchIntentFilter::Path("routes.ts".to_string()),
            SearchIntentFilter::Name("listUsers".to_string()),
            SearchIntentFilter::Language("typescript".to_string()),
        ],
    );

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].display_name, "listUsers");
    assert_eq!(hits[0].file_path.as_deref(), Some("src/routes.ts"));
}

#[test]
fn language_filter_uses_shared_registry_extensions() {
    for (requested, path) in [
        ("bash", "scripts/bootstrap.sh"),
        ("bash", "scripts/bootstrap.bash"),
        ("sh", "scripts/bootstrap.sh"),
        ("python", "pkg/types.pyi"),
        ("ts", "src/server.mts"),
        ("typescript", "src/server.cts"),
        ("dart", "lib/main.dart"),
        ("html", "templates/index.htm"),
        ("css", "assets/site.css"),
        ("sql", "db/schema.sql"),
        ("c++", "include/runtime.hh"),
        ("c#", "src/App.cs"),
        ("markdown", "docs/guide.mdx"),
    ] {
        assert!(
            language_filter_matches_path(requested, path),
            "expected language:{requested} to match {path}"
        );
    }

    assert!(!language_filter_matches_path("bash", "src/main.py"));
    assert!(!language_filter_matches_path(
        "sh",
        "scripts/bootstrap.bash"
    ));
    assert!(!language_filter_matches_path("tsx", "src/server.ts"));
    assert!(!language_filter_matches_path("jsx", "src/app.js"));

    assert!(indexed_file_matches_language_filter(
        "typescript",
        Path::new("src/Widget.tsx"),
        "tsx"
    ));
    assert!(indexed_file_matches_language_filter(
        "bash",
        Path::new("scripts/bootstrap.sh"),
        "bash"
    ));
    assert!(!indexed_file_matches_language_filter(
        "typescript",
        Path::new("src/server.ts"),
        "tsx"
    ));
}

#[test]
fn llm_doc_embed_batch_size_uses_throughput_default() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::remove(LLM_DOC_EMBED_BATCH_SIZE_ENV);

    assert_eq!(llm_doc_embed_batch_size(), 128);
}

#[test]
fn framework_route_coverage_matrix_lists_coverage_evidence_and_known_gaps() {
    let coverage = framework_route_coverage_matrix();
    let frameworks = coverage
        .iter()
        .map(|entry| entry.framework.as_str())
        .collect::<HashSet<_>>();
    for expected in [
        "express",
        "react-router",
        "sveltekit",
        "nextjs",
        "remix",
        "astro",
        "nuxt",
        "fastify",
        "koa",
        "hono",
        "nestjs",
        "django",
        "flask",
        "fastapi",
        "rails",
        "laravel",
        "spring",
        "aspnet",
        "axum",
        "actix",
        "rocket",
        "gin",
        "chi",
        "echo",
        "fiber",
        "vue-router",
    ] {
        assert!(
            frameworks.contains(expected),
            "coverage matrix missing {expected}"
        );
    }
    assert!(coverage.iter().all(|entry| {
        !entry.coverage_evidence.is_empty()
            && !entry.confidence_floor.is_empty()
            && !entry.handler_link_support.is_empty()
            && !entry.unsupported_patterns.is_empty()
            && !entry.known_gaps.is_empty()
    }));
    let express = coverage
        .iter()
        .find(|entry| entry.framework == "express")
        .expect("express coverage");
    assert_eq!(express.coverage_evidence, "tree_sitter_query_regression");
    assert_eq!(express.confidence_floor, "heuristic");
    assert!(
        express
            .handler_link_support
            .contains("direct_handler_names")
    );
    let fastify = coverage
        .iter()
        .find(|entry| entry.framework == "fastify")
        .expect("Fastify coverage");
    assert_eq!(fastify.coverage_evidence, "tree_sitter_query_regression");
    assert_eq!(fastify.confidence_floor, "heuristic");
    assert!(
        fastify
            .handler_link_support
            .contains("direct_handler_names")
    );
    assert!(
        coverage
            .iter()
            .filter(|entry| entry.language == "go")
            .all(|entry| entry.handler_link_support == "not_claimed_text_only")
    );
    let fastapi = coverage
        .iter()
        .find(|entry| entry.framework == "fastapi")
        .expect("FastAPI coverage entry");
    assert_eq!(
        fastapi.coverage_evidence,
        "validated_by_tree_sitter_query_regression"
    );
    assert_eq!(
        fastapi.handler_link_support,
        "probable_for_decorated_handler"
    );
    assert_eq!(fastapi.confidence_floor, "heuristic");
    for unsupported in [
        "path= keyword arguments",
        "head/options/api_route/websocket",
        "chained or multi-target FastAPI/APIRouter construction",
        "without module-scope construction are not claimed",
    ] {
        assert!(
            fastapi
                .unsupported_patterns
                .iter()
                .any(|pattern| pattern.contains(unsupported)),
            "FastAPI coverage should record {unsupported}"
        );
    }
}

#[test]
fn llm_doc_embed_batch_size_allows_wider_managed_batches() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(LLM_DOC_EMBED_BATCH_SIZE_ENV, "1024");

    assert_eq!(llm_doc_embed_batch_size(), 1024);
}

#[test]
fn stream_pending_llm_symbol_docs_defaults_to_enabled() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::remove(SEMANTIC_STREAM_PENDING_DOCS_ENV);
    assert!(stream_pending_llm_symbol_docs_from_env());

    let _env = EnvGuard::set(SEMANTIC_STREAM_PENDING_DOCS_ENV, "false");
    assert!(!stream_pending_llm_symbol_docs_from_env());
}

#[test]
fn semantic_stream_sort_window_defaults_to_one_batch() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::remove(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV);
    assert_eq!(semantic_stream_sort_window_batches_from_env(), 1);

    let _env = EnvGuard::set(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV, "1");
    assert_eq!(semantic_stream_sort_window_batches_from_env(), 1);

    let _env = EnvGuard::set(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV, "999");
    assert_eq!(semantic_stream_sort_window_batches_from_env(), 16);
}

#[test]
fn semantic_doc_scope_defaults_to_durable_symbols_and_all_scope_is_opt_in() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::remove(SEMANTIC_DOC_SCOPE_ENV);
    assert_eq!(
        semantic_doc_scope_from_env(),
        SemanticDocScope::DurableSymbols
    );
    assert_eq!(
        semantic_doc_scope_from_value("all"),
        SemanticDocScope::AllSymbols
    );
    assert_eq!(
        semantic_doc_scope_from_value("full"),
        SemanticDocScope::AllSymbols
    );

    assert!(llm_indexable_kind(NodeKind::FUNCTION));
    assert!(llm_indexable_kind(NodeKind::STRUCT));
    assert!(llm_indexable_kind(NodeKind::GLOBAL_VARIABLE));
    assert!(llm_indexable_kind(NodeKind::CONSTANT));
    assert!(!llm_indexable_kind(NodeKind::MODULE));
    assert!(!llm_indexable_kind(NodeKind::FIELD));
    assert!(!llm_indexable_kind(NodeKind::VARIABLE));

    assert!(llm_indexable_kind_for_scope(
        NodeKind::MODULE,
        SemanticDocScope::AllSymbols
    ));
    assert!(llm_indexable_kind_for_scope(
        NodeKind::FIELD,
        SemanticDocScope::AllSymbols
    ));
    assert!(llm_indexable_kind_for_scope(
        NodeKind::VARIABLE,
        SemanticDocScope::AllSymbols
    ));
    assert!(!llm_indexable_kind_for_scope(
        NodeKind::FILE,
        SemanticDocScope::AllSymbols
    ));
    assert!(!llm_indexable_kind_for_scope(
        NodeKind::UNKNOWN,
        SemanticDocScope::AllSymbols
    ));
    assert!(!llm_indexable_kind_for_scope(
        NodeKind::BUILTIN_TYPE,
        SemanticDocScope::AllSymbols
    ));
    for raw_kind in 0..=NodeKind::UNKNOWN as i32 {
        let kind = NodeKind::try_from(raw_kind).expect("known node kind");
        for scope in [
            SemanticDocScope::DurableSymbols,
            SemanticDocScope::AllSymbols,
        ] {
            assert_eq!(
                llm_indexable_kinds_for_scope(scope).contains(&kind),
                llm_indexable_kind_for_scope(kind, scope),
                "streamed kind filter drifted for {kind:?} in {scope:?}"
            );
        }
    }
}

#[test]
fn semantic_doc_alias_mode_defaults_to_alias_variant() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::remove(SEMANTIC_DOC_ALIAS_MODE_ENV);
    assert_eq!(
        semantic_doc_alias_mode_from_env(),
        SemanticDocAliasMode::AliasVariant
    );
    assert_eq!(
        semantic_doc_alias_mode_from_value("current_alias"),
        SemanticDocAliasMode::CurrentAlias
    );
    assert_eq!(
        semantic_doc_alias_mode_from_value("no_alias"),
        SemanticDocAliasMode::NoAlias
    );
    assert_eq!(
        semantic_doc_alias_mode_from_value("compact"),
        SemanticDocAliasMode::AliasVariant
    );
}

#[test]
fn semantic_doc_token_budget_defaults_to_safe_window() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::remove(SEMANTIC_DOC_MAX_TOKENS_ENV);

    assert_eq!(
        semantic_doc_max_tokens_from_env(),
        SEMANTIC_DOC_DEFAULT_MAX_TOKENS
    );
    assert!(semantic_doc_shape_contract().contains("max_tokens=128"));
}

fn pending_semantic_doc_for_test(node_id: i64, doc_text: &str) -> PendingLlmSymbolDoc {
    PendingLlmSymbolDoc {
        node_id: CoreNodeId(node_id),
        file_node_id: Some(CoreNodeId(1)),
        kind: NodeKind::FUNCTION,
        display_name: format!("doc_{node_id}"),
        qualified_name: None,
        file_path: None,
        start_line: None,
        end_line: None,
        doc_text: doc_text.to_string(),
        doc_hash: llm_symbol_doc_hash(doc_text),
        dense_reason: DenseAnchorReason::PublicApi,
    }
}

fn semantic_policy_node(id: i64, kind: NodeKind, name: &str, file_id: i64) -> Node {
    Node {
        id: CoreNodeId(id),
        kind,
        serialized_name: name.to_string(),
        qualified_name: Some(format!("pkg::{name}")),
        file_node_id: Some(CoreNodeId(file_id)),
        start_line: Some(1),
        end_line: Some(3),
        ..Default::default()
    }
}

fn semantic_policy_context(path: &str, node: &Node) -> SemanticDocGraphContext {
    let mut context = SemanticDocGraphContext::default();
    context.file_paths.insert(
        node.file_node_id.expect("semantic policy test file id"),
        path.to_string(),
    );
    context
}

#[test]
fn dense_policy_skips_private_trivial_helpers() {
    let node = semantic_policy_node(10, NodeKind::FUNCTION, "helper", 1);
    let context = semantic_policy_context("src/internal/helper.rs", &node);

    let reason = dense_anchor_reason_for_node(
        &context,
        &node,
        "helper",
        Some("src/internal/helper.rs"),
        "semantic_doc_version: 4\nsymbol: helper\nkind: FUNCTION\n",
        Some(AccessKind::Private),
    );

    assert_eq!(reason, None);
}

#[test]
fn dense_policy_does_not_treat_every_handler_name_as_entrypoint() {
    let node = semantic_policy_node(14, NodeKind::FUNCTION, "handler", 1);
    let context = semantic_policy_context("src/internal/request.rs", &node);

    let reason = dense_anchor_reason_for_node(
        &context,
        &node,
        "handler",
        Some("src/internal/request.rs"),
        "semantic_doc_version: 4\nsymbol: handler\nkind: FUNCTION\n",
        Some(AccessKind::Private),
    );

    assert_eq!(reason, None);
}

#[test]
fn dense_policy_only_embeds_high_signal_central_nodes() {
    let ordinary = semantic_policy_node(15, NodeKind::FUNCTION, "ordinary", 1);
    let central = semantic_policy_node(16, NodeKind::FUNCTION, "central", 1);
    let mut context = semantic_policy_context("src/internal/graph.rs", &ordinary);
    context.centrality.insert(
        ordinary.id,
        DenseAnchorCentrality {
            child_count: 2,
            related_count: 2,
            edge_count: 4,
        },
    );
    context.child_labels.insert(
        central.id,
        (0..6).map(|index| format!("child_{index}")).collect(),
    );
    context.referenced_labels.insert(
        central.id,
        (0..6).map(|index| format!("ref_{index}")).collect(),
    );
    context.centrality.insert(
        central.id,
        DenseAnchorCentrality {
            child_count: 0,
            related_count: DENSE_CENTRAL_RELATIONSHIP_THRESHOLD,
            edge_count: DENSE_CENTRAL_SCORE_THRESHOLD,
        },
    );

    assert_eq!(
        dense_anchor_reason_for_node(
            &context,
            &ordinary,
            "ordinary",
            Some("src/internal/graph.rs"),
            "semantic_doc_version: 4\nsymbol: ordinary\nkind: FUNCTION\n",
            Some(AccessKind::Private),
        ),
        None
    );
    assert_eq!(
        dense_anchor_reason_for_node(
            &context,
            &central,
            "central",
            Some("src/internal/graph.rs"),
            "semantic_doc_version: 4\nsymbol: central\nkind: FUNCTION\n",
            Some(AccessKind::Private),
        ),
        Some(DenseAnchorReason::CentralGraphNode)
    );
    assert_eq!(
        context
            .child_labels
            .get(&central.id)
            .expect("bounded child labels")
            .len(),
        6
    );
    assert_eq!(
        context
            .referenced_labels
            .get(&central.id)
            .expect("bounded related labels")
            .len(),
        6
    );
}

#[test]
fn dense_policy_keeps_low_degree_local_functions_and_variables_sparse() {
    let function = semantic_policy_node(17, NodeKind::FUNCTION, "local_fn", 1);
    let variable = semantic_policy_node(18, NodeKind::VARIABLE, "local_value", 1);
    let mut context = semantic_policy_context("src/internal/local.rs", &function);
    for node_id in [function.id, variable.id] {
        context.centrality.insert(
            node_id,
            DenseAnchorCentrality {
                child_count: 0,
                related_count: DENSE_CENTRAL_RELATIONSHIP_THRESHOLD - 1,
                edge_count: DENSE_CENTRAL_SCORE_THRESHOLD,
            },
        );
    }

    for node in [&function, &variable] {
        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                node,
                &node.serialized_name,
                Some("src/internal/local.rs"),
                "semantic_doc_version: 6\nkind: local\n",
                Some(AccessKind::Private),
            ),
            None
        );
    }
}

#[test]
fn dense_policy_classifies_public_entrypoint_and_documented_symbols() {
    let public_node = semantic_policy_node(11, NodeKind::STRUCT, "ReportBuilder", 1);
    let entrypoint_node = semantic_policy_node(12, NodeKind::FUNCTION, "main", 1);
    let documented_node = semantic_policy_node(13, NodeKind::METHOD, "parse_config", 1);
    let context = semantic_policy_context("src/lib.rs", &public_node);

    assert_eq!(
        dense_anchor_reason_for_node(
            &context,
            &public_node,
            "ReportBuilder",
            Some("src/lib.rs"),
            "semantic_doc_version: 4\nsymbol: ReportBuilder\nkind: STRUCT\n",
            Some(AccessKind::Public),
        ),
        Some(DenseAnchorReason::PublicApi)
    );
    assert_eq!(
        dense_anchor_reason_for_node(
            &context,
            &entrypoint_node,
            "main",
            Some("src/main.rs"),
            "semantic_doc_version: 4\nsymbol: main\nkind: FUNCTION\n",
            Some(AccessKind::Private),
        ),
        Some(DenseAnchorReason::Entrypoint)
    );
    assert_eq!(
        dense_anchor_reason_for_node(
            &context,
            &documented_node,
            "parse_config",
            Some("src/internal/config.rs"),
            "semantic_doc_version: 4\ncomments: parses user-visible configuration\nbody_summary: validates and normalizes the configuration before runtime startup\n",
            Some(AccessKind::Private),
        ),
        Some(DenseAnchorReason::DocumentedNontrivial)
    );
}

#[test]
fn dense_policy_classifies_cross_language_entrypoints_and_surfaces() {
    let python_app = semantic_policy_node(21, NodeKind::FUNCTION, "app", 1);
    let go_command = semantic_policy_node(22, NodeKind::FUNCTION, "run", 2);
    let csharp_program = semantic_policy_node(23, NodeKind::CLASS, "Program", 3);
    let java_application = semantic_policy_node(24, NodeKind::CLASS, "Application", 4);
    let c_header_api = semantic_policy_node(25, NodeKind::STRUCT, "ClientApi", 5);
    let python_package_api = semantic_policy_node(26, NodeKind::CLASS, "PackageClient", 6);
    let mut context = SemanticDocGraphContext::default();
    context.file_paths.insert(
        python_app.file_node_id.expect("file id"),
        "service/app.py".to_string(),
    );
    context.file_paths.insert(
        go_command.file_node_id.expect("file id"),
        "cmd/server/main.go".to_string(),
    );
    context.file_paths.insert(
        csharp_program.file_node_id.expect("file id"),
        "src/Program.cs".to_string(),
    );
    context.file_paths.insert(
        java_application.file_node_id.expect("file id"),
        "src/main/java/com/acme/Application.java".to_string(),
    );
    context.file_paths.insert(
        c_header_api.file_node_id.expect("file id"),
        "include/acme/client_api.hpp".to_string(),
    );
    context.file_paths.insert(
        python_package_api.file_node_id.expect("file id"),
        "packages/acme_sdk/__init__.py".to_string(),
    );

    for (node, display_name, file_path) in [
        (&python_app, "app", "service/app.py"),
        (&go_command, "run", "cmd/server/main.go"),
        (&csharp_program, "Program", "src/Program.cs"),
        (
            &java_application,
            "Application",
            "src/main/java/com/acme/Application.java",
        ),
    ] {
        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                node,
                display_name,
                Some(file_path),
                "semantic_doc_version: 4\nsymbol: entrypoint\nkind: FUNCTION\n",
                Some(AccessKind::Private),
            ),
            Some(DenseAnchorReason::Entrypoint),
            "{file_path} should classify as an entrypoint"
        );
    }

    for (node, display_name, file_path) in [
        (&c_header_api, "ClientApi", "include/acme/client_api.hpp"),
        (
            &python_package_api,
            "PackageClient",
            "packages/acme_sdk/__init__.py",
        ),
    ] {
        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                node,
                display_name,
                Some(file_path),
                "semantic_doc_version: 4\nsymbol: api\nkind: STRUCT\n",
                Some(AccessKind::Private),
            ),
            Some(DenseAnchorReason::PublicApi),
            "{file_path} should classify as a public surface"
        );
    }
}

#[test]
fn dense_policy_does_not_embed_plain_public_callables_by_default() {
    let node = semantic_policy_node(17, NodeKind::FUNCTION, "plain_public_function", 1);
    let context = semantic_policy_context("src/lib.rs", &node);

    let reason = dense_anchor_reason_for_node(
        &context,
        &node,
        "plain_public_function",
        Some("src/lib.rs"),
        "semantic_doc_version: 4\nsymbol: plain_public_function\nkind: FUNCTION\n",
        Some(AccessKind::Public),
    );

    assert_eq!(reason, None);
}

#[test]
fn dense_policy_embeds_package_public_callables_for_dynamic_frameworks() {
    let node = semantic_policy_node(19, NodeKind::FUNCTION, "handle", 1);
    let context = semantic_policy_context("lib/router/index.js", &node);

    let reason = dense_anchor_reason_for_node(
        &context,
        &node,
        "handle",
        Some("lib/router/index.js"),
        "semantic_doc_version: 4\nsymbol: handle\nkind: FUNCTION\nsignature: function handle(req, res, next) {}\n",
        Some(AccessKind::Private),
    );

    assert_eq!(reason, Some(DenseAnchorReason::PublicApi));

    let windows_node = semantic_policy_node(29, NodeKind::METHOD, "GET /json", 1);
    let windows_path = r"\\?\C:\repo\expressjs-express\lib\response.js";
    let windows_context = semantic_policy_context(windows_path, &windows_node);

    let windows_reason = dense_anchor_reason_for_node(
        &windows_context,
        &windows_node,
        "GET /json",
        Some(windows_path),
        "semantic_doc_version: 4\nsymbol: GET /json\nkind: METHOD\nsignature: .get('/json')\n",
        Some(AccessKind::Private),
    );

    assert_eq!(windows_reason, Some(DenseAnchorReason::PublicApi));
}

#[test]
fn dense_policy_does_not_embed_comment_only_symbols_by_default() {
    let node = semantic_policy_node(18, NodeKind::FUNCTION, "commented_helper", 1);
    let context = semantic_policy_context("src/internal/helper.rs", &node);

    let reason = dense_anchor_reason_for_node(
        &context,
        &node,
        "commented_helper",
        Some("src/internal/helper.rs"),
        "semantic_doc_version: 4\ncomments: explains how helper is used by nearby code\nsignature: fn commented_helper() {}\n",
        Some(AccessKind::Private),
    );

    assert_eq!(reason, None);
}

#[test]
fn component_reports_are_extracted_dense_anchors_with_virtual_ids() {
    let node = semantic_policy_node(20, NodeKind::FUNCTION, "central_service", 1);
    let mut context = semantic_policy_context("crates/app/src/service.rs", &node);
    context
        .edge_digests
        .insert(node.id, vec!["CALL=9".to_string()]);
    let reports =
        build_component_report_docs(&context, &[&node], &std::collections::HashMap::new(), 123);

    assert_eq!(reports.len(), 1);
    let report = &reports[0];
    assert!(report.symbol_doc.node_id.0 < 0);
    assert_eq!(report.symbol_doc.source_provenance, "extracted");
    assert_eq!(report.symbol_doc.policy_version, SEMANTIC_POLICY_VERSION);
    assert!(
        report
            .symbol_doc
            .doc_text
            .contains("component_report: crate:app")
    );
    assert_eq!(
        report.symbol_doc.file_path.as_deref(),
        Some("crates/app/src/service.rs")
    );
    assert!(report.symbol_doc.doc_text.contains("god_nodes:"));
    let pending = report
        .pending
        .as_ref()
        .expect("component report should publish a dense anchor input");
    assert_eq!(pending.node_id, report.symbol_doc.node_id);
    assert_eq!(pending.dense_reason, DenseAnchorReason::ComponentReport);
    assert_eq!(pending.doc_hash, report.symbol_doc.doc_hash);
    assert_eq!(pending.doc_text, report.symbol_doc.doc_text);
    assert!(pending.end_line.is_none());
    assert!(!report.reusable);
}

#[test]
fn component_reports_group_root_level_source_files() {
    assert_eq!(
        semantic_component_key_for_path(Some("nvm.sh")).as_deref(),
        Some("dir:.")
    );
}

#[test]
fn semantic_graph_context_keeps_normalized_paths_once_per_file() {
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    let verbatim_path = PathBuf::from(r"\\?\C:\work\nvm\nvm.sh");
    storage
        .insert_file(&FileInfo {
            id: 11,
            path: verbatim_path.clone(),
            language: "bash".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 12,
            file_role: codestory_store::FileRole::Source,
        })
        .expect("insert file");
    let file_node = Node {
        id: CoreNodeId(11),
        kind: NodeKind::FILE,
        serialized_name: verbatim_path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let function_node = Node {
        id: CoreNodeId(101),
        kind: NodeKind::FUNCTION,
        serialized_name: "nvm".to_string(),
        file_node_id: Some(CoreNodeId(11)),
        start_line: Some(1),
        ..Default::default()
    };
    let second_function_node = Node {
        id: CoreNodeId(102),
        kind: NodeKind::FUNCTION,
        serialized_name: "nvm_echo".to_string(),
        file_node_id: Some(CoreNodeId(11)),
        start_line: Some(2),
        ..Default::default()
    };
    storage
        .insert_nodes_batch(&[
            file_node.clone(),
            function_node.clone(),
            second_function_node.clone(),
        ])
        .expect("insert nodes");
    let nodes = vec![
        file_node,
        function_node.clone(),
        second_function_node.clone(),
    ];
    let semantic_nodes = vec![&function_node, &second_function_node];
    let context =
        SemanticDocGraphContext::build(&storage, &semantic_nodes, &nodes).expect("context");

    assert_eq!(context.file_paths.len(), 1);
    assert_eq!(context.file_read_paths.len(), 1);
    assert_eq!(context.file_path_for_node(&function_node), Some("nvm.sh"));
    assert_eq!(
        context.file_path_for_node(&second_function_node),
        Some("nvm.sh")
    );
    assert_eq!(
        context.file_read_path_for_node(&function_node),
        Some("C:/work/nvm/nvm.sh")
    );
    let reports = build_component_report_docs(
        &context,
        &semantic_nodes,
        &std::collections::HashMap::new(),
        123,
    );
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].symbol_doc.file_path.as_deref(), Some("nvm.sh"));
    assert!(
        reports[0]
            .symbol_doc
            .doc_text
            .contains("component_report: dir:.")
    );
}

#[test]
fn semantic_refresh_scope_includes_files_connected_to_changed_graph_nodes() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "src/caller.rs".into(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(2),
                kind: NodeKind::FILE,
                serialized_name: "src/callee.rs".into(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(11),
                kind: NodeKind::FUNCTION,
                serialized_name: "caller".into(),
                file_node_id: Some(CoreNodeId(1)),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(22),
                kind: NodeKind::FUNCTION,
                serialized_name: "callee".into(),
                file_node_id: Some(CoreNodeId(2)),
                ..Default::default()
            },
        ])
        .expect("nodes");
    storage
        .insert_edges_batch(&[Edge {
            id: EdgeId(1),
            source: CoreNodeId(11),
            target: CoreNodeId(22),
            kind: EdgeKind::CALL,
            resolved_source: Some(CoreNodeId(11)),
            resolved_target: Some(CoreNodeId(22)),
            ..Default::default()
        }])
        .expect("edge");

    let dependents =
        semantic_graph_dependent_file_ids_by_seed(&storage, &HashSet::from([CoreNodeId(1)]))
            .expect("semantic dependents");

    assert_eq!(
        dependents.get(&CoreNodeId(1)),
        Some(&HashSet::from([CoreNodeId(1), CoreNodeId(2)])),
        "an untouched endpoint file must be recomputed when related-symbol and edge text can change"
    );
}

fn semantic_file_text_cache_node(
    id: i64,
    display_path: &str,
    read_path: &Path,
    context: &mut SemanticDocGraphContext,
) -> Node {
    let node = Node {
        id: CoreNodeId(id),
        kind: NodeKind::FUNCTION,
        serialized_name: format!("symbol_{id}"),
        file_node_id: Some(CoreNodeId(id + 100)),
        start_line: Some(1),
        ..Default::default()
    };
    let file_id = node.file_node_id.expect("semantic cache test file id");
    context.file_paths.insert(file_id, display_path.to_string());
    context
        .file_read_paths
        .insert(file_id, read_path.to_string_lossy().to_string());
    node
}

#[test]
fn semantic_file_text_cache_skips_files_above_byte_limit() {
    let temp = tempdir().expect("create temp dir");
    let small_path = temp.path().join("small.rs");
    let large_path = temp.path().join("large.rs");
    fs::write(&small_path, "small").expect("write small file");
    fs::write(&large_path, "too-large").expect("write large file");
    let mut context = SemanticDocGraphContext::default();
    let nodes = [
        semantic_file_text_cache_node(1, "small.rs", &small_path, &mut context),
        semantic_file_text_cache_node(2, "large.rs", &large_path, &mut context),
    ];
    let semantic_nodes = nodes.iter().collect::<Vec<_>>();

    let cache = build_semantic_file_text_cache_with_limits(&context, &semantic_nodes, 5, 100);

    assert_eq!(
        cache
            .get("small.rs")
            .and_then(|contents| contents.as_deref()),
        Some("small")
    );
    assert_eq!(cache.get("large.rs"), Some(&None));
}

#[test]
fn semantic_file_text_cache_respects_aggregate_byte_limit() {
    let temp = tempdir().expect("create temp dir");
    let a_path = temp.path().join("a.rs");
    let b_path = temp.path().join("b.rs");
    let c_path = temp.path().join("c.rs");
    fs::write(&a_path, "aaaa").expect("write a file");
    fs::write(&b_path, "bbbb").expect("write b file");
    fs::write(&c_path, "cc").expect("write c file");
    let mut context = SemanticDocGraphContext::default();
    let nodes = [
        semantic_file_text_cache_node(1, "a.rs", &a_path, &mut context),
        semantic_file_text_cache_node(2, "b.rs", &b_path, &mut context),
        semantic_file_text_cache_node(3, "c.rs", &c_path, &mut context),
    ];
    let semantic_nodes = nodes.iter().collect::<Vec<_>>();

    let cache = build_semantic_file_text_cache_with_limits(&context, &semantic_nodes, 100, 7);

    assert_eq!(
        cache.get("a.rs").and_then(|contents| contents.as_deref()),
        Some("aaaa")
    );
    assert_eq!(cache.get("b.rs"), Some(&None));
    assert_eq!(cache.get("c.rs"), Some(&None));
}

#[test]
fn dense_anchor_inputs_are_sorted_deterministically_before_publication() {
    let mut docs = vec![
        pending_semantic_doc_for_test(1, &"x".repeat(900)),
        pending_semantic_doc_for_test(2, "tiny"),
        pending_semantic_doc_for_test(3, &"m".repeat(880)),
        pending_semantic_doc_for_test(4, "small"),
    ];
    sort_pending_dense_anchor_inputs(&mut docs);

    assert_eq!(
        docs.iter().map(|doc| doc.node_id.0).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

fn semantic_doc_text_for_test(
    display_name: &str,
    qualified_name: Option<&str>,
    file_path: &str,
    kind: NodeKind,
) -> String {
    let node = Node {
        id: CoreNodeId(10),
        kind,
        serialized_name: display_name.to_string(),
        qualified_name: qualified_name.map(str::to_string),
        file_node_id: Some(CoreNodeId(1)),
        start_line: Some(12),
        ..Default::default()
    };
    let graph_context = SemanticDocGraphContext::default();
    let file_text_cache = HashMap::new();
    build_llm_symbol_doc_text(
        &graph_context,
        &node,
        display_name,
        Some(file_path),
        &file_text_cache,
    )
}

#[test]
fn semantic_doc_text_adds_symbol_aliases_for_supported_language_naming_styles() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
    let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
    let cases = [
        (
            "rust",
            "src/game_state.rs",
            "crate::game_state::check_winner",
            Some("crate::game_state::check_winner"),
            "check winner",
            "crate game state check winner",
        ),
        (
            "python",
            "pkg/engine.py",
            "pkg.engine.build_snapshot_digest",
            Some("pkg.engine.build_snapshot_digest"),
            "build snapshot digest",
            "pkg engine build snapshot digest",
        ),
        (
            "javascript",
            "src/GameController.js",
            "GameController.checkWinner",
            Some("GameController.checkWinner"),
            "check winner",
            "game controller check winner",
        ),
        (
            "typescript",
            "src/useWinningMove.ts",
            "useWinningMove",
            None,
            "use winning move",
            "use winning move",
        ),
        (
            "java",
            "src/main/java/GameController.java",
            "com.example.GameController.checkWinner",
            Some("com.example.GameController.checkWinner"),
            "check winner",
            "com example game controller check winner",
        ),
        (
            "c",
            "src/field_ops.c",
            "field_clear_move",
            None,
            "field clear move",
            "field clear move",
        ),
        (
            "cpp",
            "src/field_ops.cpp",
            "Game::Field::clearMove",
            Some("Game::Field::clearMove"),
            "clear move",
            "game field clear move",
        ),
    ];

    for (language, file_path, display_name, qualified_name, terminal_alias, full_alias) in cases {
        let doc =
            semantic_doc_text_for_test(display_name, qualified_name, file_path, NodeKind::FUNCTION);
        assert!(
            doc.contains(&format!("language: {language}")),
            "doc should include language for {file_path}:\n{doc}"
        );
        assert!(
            doc.contains(&format!("terminal_alias: {terminal_alias}")),
            "doc should include terminal alias for {display_name}:\n{doc}"
        );
        assert!(
            doc.contains(&format!("name_aliases: {full_alias}")),
            "doc should include normalized full alias for {display_name}:\n{doc}"
        );
    }
}

#[test]
fn semantic_doc_text_adds_kind_role_owner_and_path_alias_context() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
    let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
    let doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );

    assert!(
        doc.contains(
            "symbol_role: method member function object behavior callable routine operation"
        ),
        "method docs should include callable role aliases:\n{doc}"
    );
    assert!(
        doc.contains("owner_aliases: AppController, app controller"),
        "method docs should expose owner/container aliases:\n{doc}"
    );
    assert!(
        doc.contains("terminal_alias: open project with storage path"),
        "method docs should expose normalized terminal names:\n{doc}"
    );
    assert!(
        doc.contains("path_aliases: crates, codestory-runtime, codestory runtime, src, lib"),
        "method docs should expose file path aliases:\n{doc}"
    );
}

#[test]
fn semantic_doc_text_keeps_comments_before_long_file_path() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
    let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "128");
    let file_path = r"\\?\C:\Users\alber\AppData\Local\Temp\codestory-search-quality-fixture-with-a-long-path\src\architecture.ts";
    let file_text = r#"// Project source groups create indexing commands and storage access.
export class SourceGroupCxxCdb {
  getIndexerCommands() { return []; }
}
"#;
    let node = Node {
        id: CoreNodeId(10),
        kind: NodeKind::CLASS,
        serialized_name: "SourceGroupCxxCdb".to_string(),
        qualified_name: Some("SourceGroupCxxCdb".to_string()),
        file_node_id: Some(CoreNodeId(1)),
        start_line: Some(2),
        end_line: Some(4),
        ..Default::default()
    };
    let mut file_text_cache = HashMap::new();
    file_text_cache.insert(file_path.to_string(), Some(file_text.to_string()));

    let doc = build_llm_symbol_doc_text(
        &SemanticDocGraphContext::default(),
        &node,
        "SourceGroupCxxCdb",
        Some(file_path),
        &file_text_cache,
    );

    assert!(
        doc.contains(
            "comments: // Project source groups create indexing commands and storage access."
        ),
        "symbol docs should preserve nearby comments before long file paths consume the token budget:\n{doc}"
    );
}

#[test]
fn semantic_doc_text_alias_modes_are_switchable_for_research() {
    let _lock = process_env_test_lock();
    let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
    let no_alias = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "no_alias");
    let no_alias_doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );
    let no_alias_hash = llm_symbol_doc_hash(&no_alias_doc);
    assert!(!no_alias_doc.contains("terminal_alias:"));
    assert!(!no_alias_doc.contains("path_aliases:"));
    drop(no_alias);

    let variant = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "alias_variant");
    let variant_doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );
    let variant_hash = llm_symbol_doc_hash(&variant_doc);
    assert!(variant_doc.contains("terminal_alias: open project with storage path"));
    assert!(variant_doc.contains("owner_aliases: AppController, app controller"));
    assert!(variant_doc.contains("symbol_role: method member function"));
    assert!(!variant_doc.contains("name_aliases:"));
    assert!(!variant_doc.contains("path_aliases:"));
    assert_ne!(no_alias_hash, variant_hash);
    drop(variant);

    let current = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
    let current_doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );
    assert!(current_doc.contains("name_aliases:"));
    assert!(current_doc.contains("path_aliases:"));
    assert_ne!(variant_hash, llm_symbol_doc_hash(&current_doc));
    drop(current);
}

#[test]
fn semantic_doc_text_token_budget_respects_configured_limit() {
    let _lock = process_env_test_lock();
    let _alias = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
    let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "48");
    let doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );

    assert!(
        semantic_doc_text_budget_cost(&doc) <= 48,
        "budgeted semantic doc should stay within the configured token budget:\n{doc}"
    );
    assert!(
        doc.starts_with("semantic_doc_version:"),
        "budgeted semantic doc should preserve the leading version field:\n{doc}"
    );
    assert!(
        doc.contains("symbol: AppController::openProjectWithStoragePath"),
        "budgeted semantic doc should preserve the symbol identity:\n{doc}"
    );
}

#[test]
fn semantic_doc_text_token_budget_charges_long_identifiers() {
    let doc = concat!(
        "semantic_doc_version: 1\n",
        "symbol: AppController::openProjectWithStoragePath\n",
        "path_aliases: crates codestory runtime src lib rs app controller open project ",
        "storage path AppControllerOpenProjectWithStoragePathRepeatedRepeated\n",
    );
    let truncated = truncate_semantic_doc_text_to_token_budget(doc, 36);

    assert!(
        semantic_doc_text_budget_cost(&truncated) <= 36,
        "budgeted semantic doc should stay under the conservative token proxy:\n{truncated}"
    );
    assert!(
        truncated.split_whitespace().count() < doc.split_whitespace().count(),
        "long identifier-heavy docs should be truncated earlier than whitespace counts alone"
    );
    assert!(
        truncated.contains("symbol: AppController::openProjectWithStoragePath"),
        "budgeted semantic doc should retain leading symbol identity:\n{truncated}"
    );
}

fn copy_tictactoe_workspace() -> tempfile::TempDir {
    let temp = tempdir().expect("create temp dir");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crates dir")
        .join("codestory-indexer")
        .join("tests")
        .join("fixtures")
        .join("tictactoe");

    for entry in fs::read_dir(&fixtures).expect("read fixtures") {
        let entry = entry.expect("fixture entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let target = temp.path().join(entry.file_name());
        fs::copy(&path, &target).expect("copy fixture");
    }

    temp
}

fn write_semantic_fixture(root: &std::path::Path) -> PathBuf {
    let file_path = root.join("semantic_fixture.rs");
    fs::write(
        &file_path,
        r#"
pub fn alpha() {
beta();
}

pub fn beta() {}
"#,
    )
    .expect("write semantic fixture");
    file_path
}

fn write_reindex_semantic_fixture(root: &std::path::Path, digest_text: &str) {
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    let digest_identifier = digest_text.replace(' ', "_");
    fs::write(
        src.join("lib.rs"),
        format!(
            r#"
/// {digest_text}
pub fn build_snapshot_digest({digest_identifier}: &str) -> &'static str {{
"{digest_text}"
}}

pub fn exact_symbol_anchor() {{}}
"#
        ),
    )
    .expect("write reindex fixture");
}

fn insert_semantic_fixture_nodes(storage: &mut Storage, file_path: &std::path::Path) {
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(1),
                kind: NodeKind::FILE,
                serialized_name: file_path.to_string_lossy().to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(2),
                kind: NodeKind::FUNCTION,
                serialized_name: "alpha".to_string(),
                qualified_name: Some("pkg::alpha".to_string()),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(2),
                end_line: Some(4),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(3),
                kind: NodeKind::FUNCTION,
                serialized_name: "beta".to_string(),
                qualified_name: Some("pkg::beta".to_string()),
                file_node_id: Some(CoreNodeId(1)),
                start_line: Some(6),
                end_line: Some(6),
                ..Default::default()
            },
        ])
        .expect("insert semantic fixture nodes");
}

fn test_index_publication(generation: u64, generation_id: &str) -> IndexPublicationRecord {
    IndexPublicationRecord {
        generation,
        generation_id: generation_id.to_string(),
        run_id: format!("test-run-{generation}"),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: generation as i64,
    }
}

fn persisted_search_generation_names(storage_path: &Path) -> Vec<String> {
    let root = search_index_generation_root(storage_path);
    if !root.is_dir() {
        return Vec::new();
    }
    let mut names = fs::read_dir(root)
        .expect("list persisted search generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn extract_symbol_search_terms_removes_stopwords_and_short_tokens() {
    let terms = extract_symbol_search_terms("How does the language parsing work in this repo?");
    assert_eq!(terms, vec!["language".to_string(), "parsing".to_string()]);
}

#[test]
fn broad_architecture_search_plan_terms_and_subqueries_are_bounded() {
    let query = "Explain how CodeStory's full-index path flows through CLI/runtime/workspace/indexer/store and how that supports later search, trail, and snippet commands.";
    let terms = search_plan_terms(query);
    for expected in [
        "full-index",
        "full",
        "index",
        "cli",
        "runtime",
        "workspace",
        "indexer",
        "store",
        "search",
        "trail",
        "snippet",
    ] {
        assert!(
            terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(expected)),
            "expected `{expected}` in extracted terms: {:?}",
            terms.extracted
        );
    }
    assert!(
        terms
            .dropped
            .iter()
            .any(|term| term.term.eq_ignore_ascii_case("explain")),
        "natural-language filler should be visible as dropped terms: {:?}",
        terms.dropped
    );
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");
    let subqueries = search_plan_subqueries(query, &terms, &intents);
    assert!(
        (3..=8).contains(&subqueries.len()),
        "subqueries should be bounded: {subqueries:#?}"
    );
    assert!(
        subqueries.iter().any(|subquery| subquery
            .channels
            .contains(&SearchPlanChannelDto::TypedSymbol)),
        "subqueries should cover typed symbol discovery: {subqueries:#?}"
    );
    assert!(
        subqueries
            .iter()
            .any(|subquery| subquery.channels.contains(&SearchPlanChannelDto::RepoText)),
        "subqueries should cover repo text discovery: {subqueries:#?}"
    );
}

#[test]
fn sourcetrail_style_architecture_prompt_expands_flow_roles() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application. Cite the source files that support the path.";
    let terms = search_plan_terms(query);
    assert!(
        terms
            .dropped
            .iter()
            .any(|term| term.term.eq_ignore_ascii_case("cite")),
        "citation instruction should not become a named anchor: {:?}",
        terms.dropped
    );
    for expected in [
        "BuildIndex",
        "SourceGroup",
        "IndexerCommand",
        "build",
        "index",
        "storage",
        "persistence",
    ] {
        assert!(
            terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(expected)),
            "expected inferred architecture term `{expected}` in {:?}",
            terms.extracted
        );
    }

    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let subqueries = search_plan_subqueries(query, &terms, &intents);
    assert!(
        !subqueries
            .iter()
            .any(|subquery| subquery.role == "named_anchor" && subquery.query == "Cite"),
        "generic citation wording should not consume a named-anchor slot: {subqueries:#?}"
    );
    for expected_role in [
        "build_index_entrypoint",
        "source_group_configuration",
        "indexing_work",
        "storage_access_surface",
    ] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == expected_role),
            "expected role subquery `{expected_role}` in {subqueries:#?}"
        );
    }
    let typed_anchor_terms = subqueries
        .iter()
        .find(|subquery| subquery.role == "typed_anchor_terms")
        .map(|subquery| subquery.query.as_str())
        .expect("typed anchor terms");
    for expected in ["BuildIndex", "SourceGroup", "IndexerCommand"] {
        assert!(
            typed_anchor_terms.contains(expected),
            "typed anchor terms should contain `{expected}`, got `{typed_anchor_terms}`"
        );
    }
}

#[test]
fn event_output_architecture_prompt_expands_processor_abstraction() {
    let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.";
    let terms = search_plan_terms(query);
    assert!(
        terms.extracted.iter().any(|term| term == "EventProcessor"),
        "event-output architecture prompt should infer source-truth abstraction: {:?}",
        terms.extracted
    );

    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let subqueries = search_plan_subqueries(query, &terms, &intents);
    let typed_anchor_terms = subqueries
        .iter()
        .find(|subquery| subquery.role == "typed_anchor_terms")
        .map(|subquery| subquery.query.as_str())
        .expect("typed anchor terms");
    assert!(
        typed_anchor_terms.contains("EventProcessor"),
        "typed anchor terms should include EventProcessor, got `{typed_anchor_terms}`"
    );
}

#[test]
fn multi_anchor_agent_question_prioritizes_named_anchor_subquery_terms() {
    let query = "Explain how ProjectAlpha turns configuration into processing work, then how processed data is accessed by the application. Anchor the answer around ConfigGroup, WorkerRunner, and DataAccess.";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(
        intents.iter().any(|intent| intent == "orchestration"),
        "explain-how architecture question should trigger a search plan: {intents:#?}"
    );
    let terms = search_plan_terms(query);
    for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
        assert!(
            terms.extracted.iter().any(|term| term == expected),
            "expected named anchor `{expected}` in extracted terms: {:?}",
            terms.extracted
        );
    }

    let subqueries = search_plan_subqueries(query, &terms, &intents);
    let typed_anchor_terms = subqueries
        .iter()
        .find(|subquery| subquery.role == "typed_anchor_terms")
        .map(|subquery| subquery.query.as_str())
        .expect("typed anchor subquery");
    for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
        );
        assert!(
            typed_anchor_terms.contains(expected),
            "typed anchor subquery should prioritize named anchors; got `{typed_anchor_terms}`"
        );
    }
}

#[test]
fn search_plan_still_runs_for_seed_anchor_drill_queries_with_exact_hits() {
    let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, RuntimeContext::ensure_open_from_summary, WorkspaceIndexer::run";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    assert!(
        search_plan_eligible(query, 3, &intents),
        "drill seed-anchor queries need a plan even when the anchors produce exact symbol hits"
    );

    let same_query_without_seed_anchors = "Explain how run_index RuntimeContext::ensure_open_from_summary WorkspaceIndexer::run moves through the runtime.";
    assert!(
        !search_plan_eligible(same_query_without_seed_anchors, 3, &intents),
        "ordinary exact-symbol queries should keep the exact-hit suppression"
    );
}

#[test]
fn broad_explain_how_search_plan_survives_generic_exact_hits() {
    let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    assert!(
        search_plan_eligible(query, 7, &intents),
        "generic exact hits such as CLI should not suppress broad architecture search plans"
    );
    let terms = search_plan_terms(query);
    let roles = search_plan_subqueries(query, &terms, &intents)
        .into_iter()
        .map(|subquery| subquery.role)
        .collect::<Vec<_>>();
    for expected in [
        "workspace_discovery",
        "symbol_extraction",
        "persistence_surface",
    ] {
        assert!(
            roles.iter().any(|role| role == expected),
            "broad explain-how prompt should expand architecture role `{expected}`: {roles:#?}"
        );
    }

    let ordinary_exact_query =
        "Explain how run_index RuntimeContext::ensure_open_from_summary moves through runtime.";
    assert!(
        !search_plan_eligible(ordinary_exact_query, 2, &intents),
        "ordinary exact-symbol explanations should still stay exact-first unless they name enough architecture surfaces"
    );
}

#[test]
fn search_plan_preserves_seed_anchor_line_exactly() {
    let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, run_index_once, RuntimeContext::ensure_open_from_summary, IndexService::run_indexing_blocking, AppController::run_indexing_blocking_inner, index_incremental, WorkspaceManifest::build_execution_plan, WorkspaceIndexer::run, WorkspaceIndexer::flush_projection_batch";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms, &intents);
    for expected in [
        "run_index",
        "run_index_once",
        "RuntimeContext::ensure_open_from_summary",
        "IndexService::run_indexing_blocking",
        "AppController::run_indexing_blocking_inner",
        "index_incremental",
        "WorkspaceManifest::build_execution_plan",
        "WorkspaceIndexer::run",
        "WorkspaceIndexer::flush_projection_batch",
    ] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected exact seed-anchor subquery for `{expected}`: {subqueries:#?}"
        );
    }
}

#[test]
fn public_surface_question_keeps_short_pascal_case_named_anchor() {
    let query = "Explain how public writing/social surfaces connect to Payload collections, comment auth, and the elsewhere feed. Anchor the answer around Posts, getElsewhereFeed, and getCommentAuth.";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms, &intents);
    for expected in ["Posts", "getElsewhereFeed", "getCommentAuth"] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
        );
    }
}

#[test]
fn payload_content_flow_prompt_expands_source_truth_anchors() {
    let query = "Explain how Root & Runtime public writing and social surfaces connect through Payload collections, post rendering, comment auth/submission, RSS, and the Elsewhere feed. Cite the source files that support the path.";
    let terms = search_plan_terms(query);
    for noisy in ["root", "runtime"] {
        assert!(
            !terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(noisy)),
            "brand phrase term `{noisy}` should not dominate Payload content-flow search: {:?}",
            terms.extracted
        );
        assert!(
            terms
                .dropped
                .iter()
                .any(|term| term.term.eq_ignore_ascii_case(noisy)
                    && term.reason == "brand_phrase_in_content_flow"),
            "brand phrase term `{noisy}` should be explained as dropped: {:?}",
            terms.dropped
        );
    }
    for expected in [
        "content config",
        "collection config",
        "Posts",
        "Comments",
        "social entries",
        "post page",
        "content client",
        "comment submission",
        "comment auth",
        "feed",
    ] {
        assert!(
            terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(expected)),
            "expected Payload content-flow term `{expected}` in {:?}",
            terms.extracted
        );
    }

    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let subqueries = search_plan_subqueries(query, &terms, &intents);
    let typed_anchor_terms = subqueries
        .iter()
        .find(|subquery| subquery.role == "typed_anchor_terms")
        .map(|subquery| subquery.query.as_str())
        .expect("typed anchor terms");
    for expected in ["Posts", "Comments", "feed"] {
        assert!(
            typed_anchor_terms.contains(expected),
            "typed anchor terms should include `{expected}`, got `{typed_anchor_terms}`"
        );
    }
    assert!(
        subqueries.iter().any(|subquery| {
            subquery.role == "content_surface"
                && subquery.query.to_ascii_lowercase().contains("comments")
        }),
        "content role subquery should preserve comment wording: {subqueries:#?}"
    );
    for expected_role in [
        "collection_config_surface",
        "comment_submission_surface",
        "public_feed_surface",
    ] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == expected_role),
            "expected role subquery `{expected_role}` in {subqueries:#?}"
        );
    }
    let comment_role_query = subqueries
        .iter()
        .find(|subquery| subquery.role == "comment_submission_surface")
        .map(|subquery| subquery.query.to_ascii_lowercase())
        .expect("comment submission role query");
    for expected in ["comment", "auth", "submission"] {
        assert!(
            comment_role_query.contains(expected),
            "comment role query should contain `{expected}`, got `{comment_role_query}`"
        );
    }
}

#[test]
fn codex_exec_json_prompt_expands_source_truth_anchors() {
    let query = "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output. Cite the source files that support the path.";
    let terms = search_plan_terms(query);
    for expected in [
        "EventProcessor",
        "exec cli",
        "exec runtime",
        "exec session",
        "event processor",
        "event output",
        "thread start",
        "turn start",
    ] {
        assert!(
            terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(expected)),
            "expected Codex exec-flow term `{expected}` in {:?}",
            terms.extracted
        );
    }

    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let subqueries = search_plan_subqueries(query, &terms, &intents);
    let typed_anchor_terms = subqueries
        .iter()
        .find(|subquery| subquery.role == "typed_anchor_terms")
        .map(|subquery| subquery.query.as_str())
        .expect("typed anchor terms");
    assert!(
        typed_anchor_terms.contains("EventProcessor"),
        "typed anchor terms should include EventProcessor, got `{typed_anchor_terms}`"
    );
    for expected_role in ["exec_cli_surface", "exec_event_output_surface"] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == expected_role),
            "expected role subquery `{expected_role}` in {subqueries:#?}"
        );
    }
    let exec_cli_query = subqueries
        .iter()
        .find(|subquery| subquery.role == "exec_cli_surface")
        .map(|subquery| subquery.query.to_ascii_lowercase())
        .expect("exec CLI role query");
    for expected in ["exec", "cli", "runtime"] {
        assert!(
            exec_cli_query.contains(expected),
            "exec CLI role query should contain `{expected}`, got `{exec_cli_query}`"
        );
    }
    let event_output_query = subqueries
        .iter()
        .find(|subquery| subquery.role == "exec_event_output_surface")
        .map(|subquery| subquery.query.to_ascii_lowercase())
        .expect("event output role query");
    for expected in ["event", "output", "processor"] {
        assert!(
            event_output_query.contains(expected),
            "event-output role query should contain `{expected}`, got `{event_output_query}`"
        );
    }
}

fn search_plan_test_hit(
    id: &str,
    display_name: &str,
    file_path: &Path,
    line: u32,
    origin: SearchHitOrigin,
    resolvable: bool,
) -> SearchHit {
    SearchHit {
        node_id: NodeId(id.to_string()),
        display_name: display_name.to_string(),
        kind: codestory_contracts::api::NodeKind::METHOD,
        file_path: Some(file_path.to_string_lossy().to_string()),
        line: Some(line),
        score: 1.0,
        origin,
        match_quality: None,
        resolvable,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    }
}

#[test]
fn architecture_repo_text_window_preserves_coverage_surfaces() {
    let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
    let mut hits = vec![
        search_plan_test_hit(
            "runtime-lib",
            "crates/codestory-runtime/src/lib.rs",
            Path::new("crates/codestory-runtime/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-agent",
            "crates/codestory-runtime/src/agent/orchestrator.rs",
            Path::new("crates/codestory-runtime/src/agent/orchestrator.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "cli-runtime",
            "crates/codestory-cli/src/runtime.rs",
            Path::new("crates/codestory-cli/src/runtime.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-semantic",
            "crates/codestory-runtime/src/semantic_doc_text.rs",
            Path::new("crates/codestory-runtime/src/semantic_doc_text.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-symbol",
            "crates/codestory-runtime/src/symbol_query.rs",
            Path::new("crates/codestory-runtime/src/symbol_query.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-search",
            "crates/codestory-runtime/src/search/engine.rs",
            Path::new("crates/codestory-runtime/src/search/engine.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-search-runtime",
            "crates/codestory-runtime/src/search_runtime.rs",
            Path::new("crates/codestory-runtime/src/search_runtime.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-services",
            "crates/codestory-runtime/src/services.rs",
            Path::new("crates/codestory-runtime/src/services.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "cli-args",
            "crates/codestory-cli/src/args.rs",
            Path::new("crates/codestory-cli/src/args.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-browser",
            "crates/codestory-runtime/src/browser.rs",
            Path::new("crates/codestory-runtime/src/browser.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "indexer-lib",
            "crates/codestory-indexer/src/lib.rs",
            Path::new("crates/codestory-indexer/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "storage-impl",
            "crates/codestory-store/src/storage_impl/mod.rs",
            Path::new("crates/codestory-store/src/storage_impl/mod.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
    ];

    truncate_repo_text_hits_for_query(query, &mut hits, 10);
    let paths = hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();

    assert!(paths.contains(&"crates/codestory-runtime/src/lib.rs"));
    assert!(paths.contains(&"crates/codestory-cli/src/runtime.rs"));
    assert!(paths.contains(&"crates/codestory-runtime/src/services.rs"));
    assert!(paths.contains(&"crates/codestory-indexer/src/lib.rs"));
    assert!(paths.contains(&"crates/codestory-store/src/storage_impl/mod.rs"));
    assert_eq!(paths.len(), 10);
}

#[test]
fn architecture_repo_text_window_preserves_non_crate_source_surfaces() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
    let mut hits = vec![
        search_plan_test_hit(
            "custom-command",
            "src/lib/project/SourceGroupCustomCommand.cpp",
            Path::new("src/lib/project/SourceGroupCustomCommand.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-data",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-info",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.cpp",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.cpp",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory",
            "src/lib/project/SourceGroupFactory.cpp",
            Path::new("src/lib/project/SourceGroupFactory.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory-custom",
            "src/lib/project/SourceGroupFactoryModuleCustom.cpp",
            Path::new("src/lib/project/SourceGroupFactoryModuleCustom.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "python-empty",
            "src/lib_python/project/SourceGroupPythonEmpty.cpp",
            Path::new("src/lib_python/project/SourceGroupPythonEmpty.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory-cxx",
            "src/lib_cxx/project/SourceGroupFactoryModuleCxx.cpp",
            Path::new("src/lib_cxx/project/SourceGroupFactoryModuleCxx.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-data-h",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.h",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.h",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-info-h",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.h",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.h",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory-java",
            "src/lib_java/project/SourceGroupFactoryModuleJava.cpp",
            Path::new("src/lib_java/project/SourceGroupFactoryModuleJava.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "cdb",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            Path::new("src/lib_cxx/project/SourceGroupCxxCdb.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "storage-access",
            "src/lib/data/storage/StorageAccess.h",
            Path::new("src/lib/data/storage/StorageAccess.h"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "storage-proxy",
            "src/lib/data/storage/StorageAccessProxy.cpp",
            Path::new("src/lib/data/storage/StorageAccessProxy.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
    ];

    truncate_repo_text_hits_for_query(query, &mut hits, 10);
    let paths = hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();

    assert!(paths.contains(&"src/lib_cxx/project/SourceGroupCxxCdb.cpp"));
    assert!(paths.contains(&"src/lib/data/storage/StorageAccess.h"));
    assert!(paths.contains(&"src/lib/data/storage/StorageAccessProxy.cpp"));
    assert_eq!(paths.len(), 10);
}

#[test]
fn architecture_cross_source_coverage_promotes_concrete_role_representatives() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
    let mut indexed_hits = vec![
        search_plan_test_hit(
            "persistent-h",
            "StorageAccess",
            Path::new("src/lib/data/storage/PersistentStorage.h"),
            17,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "generic-indexer",
            "Indexer",
            Path::new("src/lib/data/indexer/Indexer.h"),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "persistent-cpp",
            "PersistentStorage::PersistentStorage",
            Path::new("src/lib/data/storage/PersistentStorage.cpp"),
            32,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "project",
            "Project::isIndexing",
            Path::new("src/lib/project/Project.cpp"),
            92,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
    ];
    for index in 0..6 {
        indexed_hits.push(search_plan_test_hit(
            &format!("generic-indexer-{index}"),
            "Indexer",
            Path::new(&format!("src/lib/data/indexer/Indexer{index}.h")),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let mut indexed_candidates = indexed_hits.clone();
    indexed_candidates.push(search_plan_test_hit(
        "storage-access-h",
        "StorageAccess::~StorageAccess",
        Path::new("src/lib/data/storage/StorageAccess.h"),
        36,
        SearchHitOrigin::IndexedSymbol,
        true,
    ));

    let mut repo_text_hits = vec![search_plan_test_hit(
        "cdb-h",
        "src/lib_cxx/project/SourceGroupCxxCdb.h",
        Path::new("src/lib_cxx/project/SourceGroupCxxCdb.h"),
        1,
        SearchHitOrigin::TextMatch,
        false,
    )];
    for index in 0..9 {
        repo_text_hits.push(search_plan_test_hit(
            &format!("wizard-{index}"),
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
            Path::new(&format!(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData{index}.cpp"
            )),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }
    let mut repo_text_candidates = repo_text_hits.clone();
    repo_text_candidates.push(search_plan_test_hit(
        "indexer-java",
        "src/lib_java/data/indexer/IndexerJava.cpp",
        Path::new("src/lib_java/data/indexer/IndexerJava.cpp"),
        15,
        SearchHitOrigin::TextMatch,
        false,
    ));

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let indexed_paths = indexed_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();

    for expected in [
        "src/lib/project/Project.cpp",
        "src/lib/data/storage/PersistentStorage.cpp",
        "src/lib/data/storage/PersistentStorage.h",
        "src/lib/data/storage/StorageAccess.h",
    ] {
        assert!(
            indexed_paths.contains(&expected),
            "expected indexed path `{expected}` in {indexed_paths:#?}"
        );
    }
    for expected in [
        "src/lib_cxx/project/SourceGroupCxxCdb.h",
        "src/lib_java/data/indexer/IndexerJava.cpp",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected repo-text path `{expected}` in {repo_text_paths:#?}"
        );
    }
    assert_eq!(indexed_hits.len(), 10);
    assert_eq!(repo_text_hits.len(), 10);
}

#[test]
fn architecture_cross_source_coverage_uses_replacement_budget_for_actual_admissions() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
    let mut indexed_hits = Vec::new();
    let indexed_candidates = Vec::new();
    let mut repo_text_hits = (0..10)
        .map(|index| {
            search_plan_test_hit(
                &format!("generic-source-group-{index}"),
                &format!("src/lib/project/SourceGroupGeneric{index}.cpp"),
                Path::new(&format!("src/lib/project/SourceGroupGeneric{index}.cpp")),
                1,
                SearchHitOrigin::TextMatch,
                false,
            )
        })
        .collect::<Vec<_>>();
    let mut repo_text_candidates = repo_text_hits.clone();
    for (id, path) in [
        (
            "source-group-cdb-h",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
        ),
        (
            "source-group-cdb-cpp",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        ),
        (
            "indexer-command-cxx-cpp",
            "src/lib_cxx/data/indexer/IndexerCommandCxx.cpp",
        ),
        (
            "indexer-command-cxx-h",
            "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
        ),
        ("indexer-java", "src/lib_java/data/indexer/IndexerJava.cpp"),
        (
            "storage-proxy",
            "src/lib/data/storage/StorageAccessProxy.cpp",
        ),
    ] {
        repo_text_candidates.push(search_plan_test_hit(
            id,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        "src/lib_java/data/indexer/IndexerJava.cpp",
        "src/lib/data/storage/StorageAccessProxy.cpp",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected high-coverage late candidate `{expected}` in {repo_text_paths:#?}"
        );
    }
    assert_eq!(repo_text_paths.len(), 10);
}

#[test]
fn search_plan_rejected_hits_exposes_repo_text_coverage_candidates() {
    let chosen = search_plan_test_hit(
        "project",
        "Project::isIndexing",
        Path::new("src/lib/project/Project.cpp"),
        92,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let anchor_groups = vec![SearchPlanAnchorGroupDto {
        anchor: "Project::isIndexing".to_string(),
        chosen_symbol: Some(chosen),
        supporting_hits: Vec::new(),
        promotion_status: SearchPlanPromotionStatusDto::TypedAnchor,
        promotion_method: None,
        caller_count: 0,
        definition_only: false,
        no_visible_callers: false,
        confidence: "high".to_string(),
        reasons: Vec::new(),
    }];
    let indexed_hits = vec![search_plan_test_hit(
        "storage-access",
        "StorageAccess::~StorageAccess",
        Path::new("src/lib/data/storage/StorageAccess.h"),
        36,
        SearchHitOrigin::IndexedSymbol,
        true,
    )];
    let repo_text_hits = vec![search_plan_test_hit(
        "source-group-cdb",
        "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        Path::new("src/lib_cxx/project/SourceGroupCxxCdb.cpp"),
        1,
        SearchHitOrigin::TextMatch,
        false,
    )];

    let rejected = search_plan_rejected_hits(&anchor_groups, &[], &indexed_hits, &repo_text_hits);

    let repo_text = rejected
        .iter()
        .find(|hit| hit.origin == SearchHitOrigin::TextMatch)
        .expect("repo-text rejected hit should be retained for diagnostics");
    assert_eq!(
        repo_text.file_path.as_deref(),
        Some("src/lib_cxx/project/SourceGroupCxxCdb.cpp")
    );
    assert!(
        repo_text.reason.contains("source=repo_text")
            && repo_text
                .reason
                .contains("coverage_key=source_group:configuration:impl")
            && repo_text.reason.contains("coverage_score=10"),
        "repo-text rejection reason should include coverage provenance: {repo_text:#?}"
    );
}

#[test]
fn architecture_coverage_promotes_exec_flow_source_surfaces() {
    let expected = [
        (
            "codex-rs/cli/src/main.rs",
            "cli:top_level_entrypoint:impl",
            8,
        ),
        (
            "codex-rs/exec/src/main.rs",
            "exec:binary_entrypoint:impl",
            9,
        ),
        ("codex-rs/exec/src/cli.rs", "exec:cli_options:impl", 10),
        ("codex-rs/exec/src/lib.rs", "exec:runtime:impl", 9),
        ("codex-rs/exec/src/exec_events.rs", "exec:events:impl", 9),
        (
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            "exec:jsonl_event_processor:impl",
            9,
        ),
        (
            "codex-rs/exec/src/event_processor.rs",
            "exec:event_processor:impl",
            8,
        ),
    ];

    for (path, expected_key, expected_score) in expected {
        let hit = search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        );
        let coverage = architecture_coverage_for_hit(&hit)
            .unwrap_or_else(|| panic!("expected coverage for {path}"));
        assert_eq!(coverage.key, expected_key);
        assert_eq!(coverage.score, expected_score);
    }
}

#[test]
fn architecture_coverage_promotes_payload_content_flow_surfaces() {
    let expected = [
        ("src/payload.config.ts", "payload:config:impl", 9),
        (
            "src/collections/Posts.ts",
            "payload:posts_collection:impl",
            10,
        ),
        (
            "src/collections/Comments.ts",
            "payload:comments_collection:impl",
            10,
        ),
        (
            "src/app/(frontend)/posts/[slug]/comments/route.ts",
            "comments:submission_route:impl",
            10,
        ),
        ("src/app/feed.xml/route.ts", "feed:rss_route:impl", 10),
        ("src/lib/payload.ts", "payload:client:impl", 10),
        (
            "src/lib/content-data/post-content.ts",
            "content:post_data:impl",
            10,
        ),
        (
            "src/lib/content-data/comment-content.ts",
            "content:comment_data:impl",
            10,
        ),
    ];

    for (path, expected_key, expected_score) in expected {
        let hit = search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        );
        let coverage = architecture_coverage_for_hit(&hit)
            .unwrap_or_else(|| panic!("expected coverage for {path}"));
        assert_eq!(coverage.key, expected_key);
        assert_eq!(coverage.score, expected_score);
    }
}

#[test]
fn architecture_cross_source_coverage_admits_late_payload_content_surfaces() {
    let query = "Explain how Root & Runtime public writing and social surfaces connect through Payload collections, post rendering, comment auth/submission, RSS, and the Elsewhere feed.";
    let mut indexed_hits = Vec::new();
    let indexed_candidates = Vec::new();
    let mut repo_text_hits = (0..10)
        .map(|index| {
            search_plan_test_hit(
                &format!("generic-payload-{index}"),
                &format!("src/app/(payload)/admin/importMap{index}.js"),
                Path::new(&format!("src/app/(payload)/admin/importMap{index}.js")),
                1,
                SearchHitOrigin::TextMatch,
                false,
            )
        })
        .collect::<Vec<_>>();
    let mut repo_text_candidates = repo_text_hits.clone();
    for path in [
        "src/collections/Posts.ts",
        "src/collections/Comments.ts",
        "src/app/(frontend)/posts/[slug]/comments/route.ts",
        "src/app/feed.xml/route.ts",
        "src/lib/payload.ts",
        "src/lib/content-data/post-content.ts",
        "src/lib/content-data/comment-content.ts",
    ] {
        repo_text_candidates.push(search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "src/collections/Posts.ts",
        "src/collections/Comments.ts",
        "src/app/(frontend)/posts/[slug]/comments/route.ts",
        "src/app/feed.xml/route.ts",
        "src/lib/payload.ts",
        "src/lib/content-data/post-content.ts",
        "src/lib/content-data/comment-content.ts",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected late Payload content surface `{expected}` in {repo_text_paths:#?}"
        );
    }
    assert_eq!(repo_text_paths.len(), 10);
}

#[test]
fn architecture_cross_source_coverage_admits_late_exec_flow_surfaces() {
    let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime and JSONL event output.";
    let mut indexed_hits = vec![search_plan_test_hit(
        "exec-cli",
        "Cli",
        Path::new("codex-rs/exec/src/cli.rs"),
        14,
        SearchHitOrigin::IndexedSymbol,
        true,
    )];
    for index in 0..9 {
        indexed_hits.push(search_plan_test_hit(
            &format!("generic-cli-{index}"),
            "Cli",
            Path::new(&format!("codex-rs/generic-{index}/src/cli.rs")),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let indexed_candidates = indexed_hits.clone();

    let mut repo_text_hits = vec![search_plan_test_hit(
        "exec-events",
        "codex-rs/exec/src/exec_events.rs",
        Path::new("codex-rs/exec/src/exec_events.rs"),
        8,
        SearchHitOrigin::TextMatch,
        false,
    )];
    for index in 0..9 {
        repo_text_hits.push(search_plan_test_hit(
            &format!("generic-client-{index}"),
            &format!("codex-rs/generic-{index}/src/client.rs"),
            Path::new(&format!("codex-rs/generic-{index}/src/client.rs")),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }
    let mut repo_text_candidates = repo_text_hits.clone();
    for path in [
        "codex-rs/cli/src/main.rs",
        "codex-rs/exec/src/main.rs",
        "codex-rs/exec/src/lib.rs",
    ] {
        repo_text_candidates.push(search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "codex-rs/exec/src/exec_events.rs",
        "codex-rs/cli/src/main.rs",
        "codex-rs/exec/src/main.rs",
        "codex-rs/exec/src/lib.rs",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected exec-flow surface `{expected}` in {repo_text_paths:#?}"
        );
    }
}

#[test]
fn architecture_cross_source_coverage_admits_late_indexed_exec_flow_surfaces() {
    let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime and JSONL event output.";
    let mut indexed_hits = vec![
        search_plan_test_hit(
            "cli-main",
            "Subcommand::Exec",
            Path::new("codex-rs/cli/src/main.rs"),
            120,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "exec-lib",
            "run_exec_session",
            Path::new("codex-rs/exec/src/lib.rs"),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
    ];
    for index in 0..8 {
        indexed_hits.push(search_plan_test_hit(
            &format!("app-server-noise-{index}"),
            "CommandExec",
            Path::new(&format!(
                "codex-rs/app-server-protocol/src/protocol/v2/noise_{index}.rs"
            )),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let mut indexed_candidates = indexed_hits.clone();
    for (id, name, path) in [
        ("exec-cli", "Cli", "codex-rs/exec/src/cli.rs"),
        ("exec-main", "clap::Parser", "codex-rs/exec/src/main.rs"),
        (
            "exec-jsonl",
            "EventProcessorWithJsonOutput::emit",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
        ),
        (
            "exec-events",
            "codex_protocol::models::WebSearchAction",
            "codex-rs/exec/src/exec_events.rs",
        ),
    ] {
        indexed_candidates.push(search_plan_test_hit(
            id,
            name,
            Path::new(path),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let mut repo_text_hits = Vec::new();

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &[],
        10,
    );

    let indexed_paths = indexed_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "codex-rs/exec/src/cli.rs",
        "codex-rs/exec/src/main.rs",
        "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
        "codex-rs/exec/src/exec_events.rs",
    ] {
        assert!(
            indexed_paths.contains(&expected),
            "expected late indexed exec-flow surface `{expected}` in {indexed_paths:#?}"
        );
    }
    assert_eq!(indexed_paths.len(), 10);
}

#[test]
fn repo_text_window_does_not_diversify_non_architecture_queries() {
    let mut hits = vec![
        search_plan_test_hit(
            "first",
            "first",
            Path::new("crates/codestory-runtime/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "second",
            "second",
            Path::new("crates/codestory-indexer/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "third",
            "third",
            Path::new("crates/codestory-store/src/storage_impl/mod.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
    ];

    truncate_repo_text_hits_for_query("run_index", &mut hits, 2);

    assert_eq!(
        hits.iter()
            .map(|hit| hit.node_id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[test]
fn search_plan_anchor_groups_keep_diverse_names_before_truncation() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("flow.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(&source_path, "fn placeholder() {}\n").expect("write source");
    let mut hits = (0..10)
        .map(|index| {
            search_plan_test_hit(
                &format!("cli-{index}"),
                "cli",
                &source_path,
                index + 1,
                SearchHitOrigin::IndexedSymbol,
                true,
            )
        })
        .collect::<Vec<_>>();
    hits.push(search_plan_test_hit(
        "workspace",
        "WorkspaceManifest::build_execution_plan",
        &source_path,
        20,
        SearchHitOrigin::IndexedSymbol,
        true,
    ));
    hits.push(search_plan_test_hit(
        "indexer",
        "WorkspaceIndexer::run",
        &source_path,
        21,
        SearchHitOrigin::IndexedSymbol,
        true,
    ));

    let terms = search_plan_terms(
        "Explain how the CLI runtime workspace indexer store and search flow fits together.",
    );
    let groups = search_plan_anchor_groups(
        "Explain how the CLI runtime workspace indexer store and search flow fits together.",
        &terms,
        &hits,
        &[],
        &[],
        &HashMap::new(),
    );
    let anchors = groups
        .iter()
        .map(|group| group.anchor.as_str())
        .collect::<Vec<_>>();
    assert!(
        anchors
            .iter()
            .any(|anchor| anchor.contains("WorkspaceManifest")),
        "duplicate cli anchors should not crowd out workspace anchor: {anchors:#?}"
    );
    assert!(
        anchors
            .iter()
            .any(|anchor| anchor.contains("WorkspaceIndexer")),
        "duplicate cli anchors should not crowd out indexer anchor: {anchors:#?}"
    );
}

#[test]
fn search_plan_ranks_active_callers_above_definition_only_anchors() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("feed.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "pub fn getLatestSocialEntries() {}\npub fn getElsewhereFeed() {}\n",
    )
    .expect("write source");
    let active = search_plan_test_hit(
        "active",
        "getLatestSocialEntries",
        &source_path,
        1,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let definition_only = search_plan_test_hit(
        "definition",
        "getElsewhereFeed",
        &source_path,
        2,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let query = "getElsewhereFeed latest social feed";
    let terms = search_plan_terms(query);
    let active_path_evidence = HashMap::from([
        (
            active.node_id.clone(),
            SearchPlanActivePathEvidence { caller_count: 2 },
        ),
        (
            definition_only.node_id.clone(),
            SearchPlanActivePathEvidence { caller_count: 0 },
        ),
    ]);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[definition_only, active],
        &[],
        &[],
        &active_path_evidence,
    );

    assert_eq!(
        groups
            .first()
            .and_then(|group| group.chosen_symbol.as_ref())
            .map(|hit| hit.display_name.as_str()),
        Some("getLatestSocialEntries"),
        "visible production callers should outrank a definition-only exact-name anchor: {groups:#?}"
    );
    assert!(
        groups.iter().any(|group| {
            group.anchor == "getElsewhereFeed"
                && group.caller_count == 0
                && group.definition_only
                && group.no_visible_callers
                && group
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("no visible production callers"))
        }),
        "definition-only callable anchors should be labeled: {groups:#?}"
    );
}

#[test]
fn search_plan_test_file_names_are_not_visible_production_callers() {
    for path in [
        "src/api.test.ts",
        "src/api.spec.ts",
        "src/api.test.tsx",
        "src/api.spec.jsx",
        "src/__tests__/api.ts",
    ] {
        assert!(
            search_plan_path_is_test_or_bench(path),
            "{path} should be treated as test code for active-path evidence"
        );
    }
}

#[test]
fn search_plan_repo_text_owner_identifier_does_not_promote_member_symbol() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("lib.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "pub struct WorkspaceIndexer;\n\nimpl WorkspaceIndexer {\n    pub fn normalize_index_path(&self) {}\n}\n\n\n\n// WorkspaceIndexer coordinates indexing flow\n",
    )
    .expect("write source");
    let member_hit = search_plan_test_hit(
        "member",
        "WorkspaceIndexer::normalize_index_path",
        &source_path,
        4,
        SearchHitOrigin::IndexedSymbol,
        false,
    );
    let repo_hit = search_plan_test_hit(
        "repo",
        "src/lib.rs:9",
        &source_path,
        9,
        SearchHitOrigin::TextMatch,
        false,
    );
    let query = "WorkspaceIndexer indexing flow";
    let terms = search_plan_terms(query);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[],
        &[repo_hit],
        &[member_hit],
        &HashMap::new(),
    );

    assert!(
        groups.iter().any(|group| {
            group.chosen_symbol.is_none()
                && matches!(
                    group.promotion_status,
                    SearchPlanPromotionStatusDto::Ambiguous
                )
        }),
        "owner-only repo-text mention should stay unbound instead of promoting to a member: {groups:#?}"
    );
}

#[test]
fn search_plan_repo_text_exact_terminal_identifier_promotes_member_symbol() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("lib.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "pub struct WorkspaceIndexer;\n\nimpl WorkspaceIndexer {\n    pub fn normalize_index_path(&self) {}\n}\n\n\n\n// normalize_index_path normalizes storage keys before indexing\n",
    )
    .expect("write source");
    let member_hit = search_plan_test_hit(
        "member",
        "WorkspaceIndexer::normalize_index_path",
        &source_path,
        4,
        SearchHitOrigin::IndexedSymbol,
        false,
    );
    let repo_hit = search_plan_test_hit(
        "repo",
        "src/lib.rs:9",
        &source_path,
        9,
        SearchHitOrigin::TextMatch,
        false,
    );
    let query = "normalize_index_path storage keys";
    let terms = search_plan_terms(query);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[],
        &[repo_hit],
        &[member_hit],
        &HashMap::new(),
    );

    assert!(
        groups.iter().any(|group| {
            group
                .chosen_symbol
                .as_ref()
                .is_some_and(|hit| hit.display_name == "WorkspaceIndexer::normalize_index_path")
                && group.promotion_method.as_deref() == Some("same_file_exact_identifier")
        }),
        "exact terminal identifier should still promote to the matching member: {groups:#?}"
    );
    let next_actions = search_plan_next_actions(&groups);
    assert!(next_actions.iter().any(|action| {
        action.action == "snippet"
            && action.node_id.0 == "member"
            && action
                .options
                .iter()
                .any(|option| option == "function_body")
    }));
}

#[test]
fn search_plan_speculation_policy_matches_hidden_trail_edges() {
    assert!(search_plan_runtime_call_is_speculative(
        Some(codestory_contracts::graph::ResolutionCertainty::Probable),
        Some(0.70)
    ));
    assert!(search_plan_runtime_call_is_speculative(None, Some(0.84)));
    assert!(!search_plan_runtime_call_is_speculative(
        Some(codestory_contracts::graph::ResolutionCertainty::Certain),
        Some(codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN)
    ));
}

#[test]
fn repo_explanation_overview_replacement_is_generic_only() {
    assert!(AppController::is_repo_explanation_search_query(
        "Explain how this repo fits together"
    ));
    assert!(!query_has_symbol_or_literal_signal(
        "Explain how this repo fits together"
    ));
    assert!(query_has_symbol_or_literal_signal(
        "Explain how AppController fits into this repo"
    ));
    assert!(query_has_symbol_or_literal_signal(
        "Explain `CODESTORY_EMBED_ALLOW_CPU` in this repo"
    ));
    assert!(query_has_symbol_or_literal_signal(
        "Explain crates/codestory-runtime/src/lib.rs in this repo"
    ));
}

#[test]
fn file_text_matching_prefers_high_signal_identifier_literals() {
    let contents = r#"
pub const CODESTORY_EMBED_ALLOW_CPU: &str = "1";

fn build_llm_symbol_doc_text() -> String {
String::new()
}
"#;

    assert_eq!(
        file_text_match_line(
            contents,
            "Where is `build_llm_symbol_doc_text` defined?",
            &extract_symbol_search_terms("Where is `build_llm_symbol_doc_text` defined?")
        ),
        Some(4)
    );
    assert_eq!(
        file_text_match_line(
            contents,
            "What sets CODESTORY_EMBED_ALLOW_CPU?",
            &extract_symbol_search_terms("What sets CODESTORY_EMBED_ALLOW_CPU?")
        ),
        Some(2)
    );
}

#[test]
fn should_expand_symbol_query_for_sentence_prompts() {
    assert!(should_expand_symbol_query(
        "How does the language parsing work in this repo?",
        0
    ));
    assert!(!should_expand_symbol_query("parser", 0));
    assert!(!should_expand_symbol_query(
        "how does the language parsing work in this repo",
        5
    ));
    assert!(!should_expand_symbol_query(
        "How does the language parsing work in this repo?",
        5
    ));
}

#[test]
fn aggregate_symbol_matches_prioritizes_direct_matches() {
    let direct = vec![(CoreNodeId(7), 2.0)];
    let expanded = vec![(CoreNodeId(7), 99.0), (CoreNodeId(8), 95.0)];
    let merged = crate::support::aggregate_symbol_matches(direct, expanded);
    assert_eq!(merged.first().map(|(id, _)| *id), Some(CoreNodeId(7)));
}

#[test]
fn build_search_hit_prefers_declaration_coordinates_and_filters_unknown_nodes() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(10),
                kind: NodeKind::FILE,
                serialized_name: "src/lib.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(11),
                kind: NodeKind::FUNCTION,
                serialized_name: "check_winner".to_string(),
                file_node_id: Some(CoreNodeId(10)),
                start_line: Some(42),
                start_col: Some(5),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(12),
                kind: NodeKind::UNKNOWN,
                serialized_name: "check_winner".to_string(),
                file_node_id: Some(CoreNodeId(10)),
                start_line: Some(99),
                ..Default::default()
            },
        ])
        .expect("insert nodes");
    storage
        .insert_occurrences_batch(&[Occurrence {
            element_id: 11,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: CoreNodeId(10),
                start_line: 87,
                start_col: 9,
                end_line: 87,
                end_col: 20,
            },
        }])
        .expect("insert occurrences");

    let node_names = HashMap::from([
        (CoreNodeId(11), "check_winner".to_string()),
        (CoreNodeId(12), "check_winner".to_string()),
    ]);

    let definition_hit =
        AppController::build_search_hit(&storage, &node_names, CoreNodeId(11), 1.0)
            .expect("provenance lookup")
            .expect("definition hit");
    assert_eq!(definition_hit.file_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(definition_hit.line, Some(42));

    assert!(
        AppController::build_search_hit(&storage, &node_names, CoreNodeId(12), 1.0)
            .expect("provenance lookup")
            .is_none(),
        "unknown placeholder nodes should be dropped from indexed results"
    );
}

#[test]
fn build_search_hit_fails_closed_when_structural_provenance_cannot_be_read() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[Node {
            id: CoreNodeId(13),
            kind: NodeKind::FUNCTION,
            serialized_name: "handler".to_string(),
            ..Default::default()
        }])
        .expect("insert node");
    storage
        .get_connection()
        .execute("DROP TABLE structural_text_unit", [])
        .expect("inject structural provenance read failure");

    let error = AppController::build_search_hit(
        &storage,
        &HashMap::from([(CoreNodeId(13), "handler".to_string())]),
        CoreNodeId(13),
        1.0,
    )
    .expect_err("provenance storage failures must abort indexed-symbol search");
    assert!(
        error
            .message
            .contains("Failed to load structural provenance for node 13")
    );
}

#[test]
fn build_search_hit_adjusts_route_scores_by_extraction_provenance() {
    fn route_canonical_id(extraction: &str) -> String {
        format!(
            "route_endpoint:{}",
            serde_json::json!({
                "kind": "framework_route",
                "framework": "express",
                "method": "GET",
                "path": "/api/users",
                "provenance": [format!("extraction:{extraction}")],
            })
        )
    }

    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(20),
                kind: NodeKind::FILE,
                serialized_name: "src/routes.ts".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(22),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("ast_indexed")),
                start_line: Some(3),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(23),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("text_only")),
                start_line: Some(3),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(24),
                kind: NodeKind::FUNCTION,
                serialized_name: "plain_handler".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                start_line: Some(8),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(25),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("tree_sitter_query")),
                start_line: Some(4),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(26),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("lexical_fallback")),
                start_line: Some(5),
                ..Default::default()
            },
        ])
        .expect("insert route nodes");
    let node_names = HashMap::from([
        (CoreNodeId(22), "GET /api/users".to_string()),
        (CoreNodeId(23), "GET /api/users".to_string()),
        (CoreNodeId(24), "plain_handler".to_string()),
        (CoreNodeId(25), "GET /api/users".to_string()),
        (CoreNodeId(26), "GET /api/users".to_string()),
    ]);

    let ast = AppController::build_search_hit(&storage, &node_names, CoreNodeId(22), 1.0)
        .expect("provenance lookup")
        .expect("ast route hit");
    let text_only = AppController::build_search_hit(&storage, &node_names, CoreNodeId(23), 1.0)
        .expect("provenance lookup")
        .expect("text-only route hit");
    let normal = AppController::build_search_hit(&storage, &node_names, CoreNodeId(24), 1.0)
        .expect("provenance lookup")
        .expect("normal hit");
    let tree_sitter = AppController::build_search_hit(&storage, &node_names, CoreNodeId(25), 1.0)
        .expect("provenance lookup")
        .expect("tree-sitter route hit");
    let lexical_fallback =
        AppController::build_search_hit(&storage, &node_names, CoreNodeId(26), 1.0)
            .expect("provenance lookup")
            .expect("lexical fallback route hit");

    assert!(
        ast.score > text_only.score,
        "AST-indexed route evidence should outrank otherwise equivalent text-only route guesses"
    );
    assert!(ast.score > normal.score);
    assert!(text_only.score < normal.score);
    assert_eq!(tree_sitter.score, ast.score);
    assert_eq!(lexical_fallback.score, text_only.score);
    assert_eq!(normal.score, 1.0);
    assert_eq!(
        normal.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
        "a valid missing unit remains resolved graph evidence"
    );

    let mut hits = [text_only, ast.clone()];
    hits.sort_by(|left, right| compare_search_hits("/api/users", left, right));
    assert_eq!(hits.first().map(|hit| &hit.node_id), Some(&ast.node_id));
}

#[test]
fn build_search_hit_marks_openapi_endpoints_as_diagnostic_source() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(30),
                kind: NodeKind::FILE,
                serialized_name: "openapi.json".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(31),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(30)),
                canonical_id: Some("openapi:endpoint:GET /api/users".to_string()),
                start_line: Some(7),
                ..Default::default()
            },
        ])
        .expect("insert OpenAPI nodes");
    let node_names = HashMap::from([(CoreNodeId(31), "GET /api/users".to_string())]);

    let hit = AppController::build_search_hit(&storage, &node_names, CoreNodeId(31), 1.0)
        .expect("provenance lookup")
        .expect("OpenAPI endpoint hit");

    assert_eq!(
        hit.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
    );
    assert_eq!(
        hit.resolution_status,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
    );
    assert_eq!(
        hit.evidence_producer.as_deref(),
        Some("openapi_endpoint_schema")
    );
    assert_eq!(hit.eligible_for_sufficiency, Some(false));
}

#[test]
fn build_search_hit_marks_generic_structural_collectors_as_non_sufficient() {
    let mut storage = Storage::new_in_memory().expect("storage");
    let source_hash = "a".repeat(64);
    let unit = codestory_store::StructuralTextUnit {
        node_id: CoreNodeId(41),
        file_id: 40,
        placement_id: "b".repeat(64),
        content_hash: "c".repeat(64),
        source_content_hash: source_hash.clone(),
        descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: "structural_markdown_collector".to_string(),
        evidence_tier: "structural_text".to_string(),
        resolution: "source_range_only".to_string(),
        language: "markdown".to_string(),
        kind: NodeKind::MODULE,
        start_line: 2,
        start_col: 1,
        end_line: 2,
        end_col: 4,
        file_role: codestory_store::FileRole::Source,
    };
    storage
        .projections()
        .flush_projection_batch(codestory_store::ProjectionBatch {
            files: &[codestory_store::FileInfo {
                id: 40,
                path: PathBuf::from("docs/demo.md"),
                language: "markdown".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 2,
                file_role: codestory_store::FileRole::Source,
            }],
            file_content_hashes: &[codestory_store::FileContentHash {
                file_id: 40,
                content_hash: source_hash.clone(),
            }],
            nodes: &[
                Node {
                    id: CoreNodeId(40),
                    kind: NodeKind::FILE,
                    serialized_name: "docs/demo.md".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(41),
                    kind: NodeKind::MODULE,
                    serialized_name: "demo".to_string(),
                    file_node_id: Some(CoreNodeId(40)),
                    start_line: Some(2),
                    start_col: Some(1),
                    end_line: Some(2),
                    end_col: Some(4),
                    ..Default::default()
                },
            ],
            structural_text_units: std::slice::from_ref(&unit),
            structural_text_projections: &[codestory_store::StructuralTextProjection {
                file_id: 40,
                source_content_hash: source_hash,
                descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
                producer: "structural_markdown_collector".to_string(),
                language: "markdown".to_string(),
                file_role: codestory_store::FileRole::Source,
                unit_count: 1,
                unit_digest: codestory_store::structural_text_unit_digest(std::slice::from_ref(
                    &unit,
                )),
            }],
            structural_text_cache_writes: &[],
            edges: &[],
            occurrences: &[],
            component_access: &[],
            callable_projection_states: &[],
            file_errors: &[],
        })
        .expect("insert verified structural projection");
    let node_names = HashMap::from([(CoreNodeId(41), "demo".to_string())]);

    let hit = AppController::build_search_hit(&storage, &node_names, CoreNodeId(41), 1.0)
        .expect("provenance lookup")
        .expect("structural hit");

    assert_eq!(
        hit.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
    );
    assert_eq!(
        hit.evidence_producer.as_deref(),
        Some("structural_markdown_collector")
    );
    assert_eq!(
        hit.resolution_status,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
    );
    assert_eq!(hit.eligible_for_sufficiency, Some(false));
}

#[test]
fn indexed_files_reports_incomplete_reason_counts() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let unknown_path = temp.path().join("src").join("unknown.rs");
    let error_path = temp.path().join("src").join("error.rs");
    std::fs::create_dir_all(unknown_path.parent().expect("src parent")).expect("create src");
    std::fs::write(&unknown_path, "fn unknown() {}\n").expect("write unknown");
    std::fs::write(&error_path, "fn broken( {\n").expect("write error");

    {
        let mut storage = Storage::open(&storage_path).expect("open storage");
        for (id, path) in [(11, unknown_path), (12, error_path)] {
            let file = FileInfo {
                id,
                path,
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: false,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            };
            storage.insert_file(&file).expect("insert file");
            if id == 11 {
                storage
                    .update_file_metadata(&file, Some("verified-partial"))
                    .expect("persist verified content hash");
            }
        }
        storage
            .insert_error(&codestory_contracts::graph::ErrorInfo {
                message: "parse failed".to_string(),
                file_id: Some(CoreNodeId(12)),
                line: Some(1),
                column: Some(1),
                is_fatal: false,
                index_step: codestory_contracts::graph::IndexStep::Indexing,
                coverage_reason: Some(FileCoverageReason::CollectorFailure),
            })
            .expect("insert error");
        let publication = test_index_publication(1, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");
        let identity = project_identity_v3(temp.path());
        storage
            .publish_source_policy_exclusion_generation(
                &publication,
                &identity.project_id,
                &identity.workspace_id,
                default_source_policy_identity(),
                &[],
            )
            .expect("publish source policy identity");
        storage
            .publish_structural_text_unit_generation(&publication)
            .expect("publish structural text identity");
        storage
            .put_index_publication(&publication)
            .expect("publish complete core identity");
    }

    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project");
    let output = controller
        .indexed_files(IndexedFilesRequest {
            path_contains: None,
            language: None,
            role: None,
            limit: Some(50),
        })
        .expect("indexed files");

    assert_eq!(output.summary.incomplete_file_count, 2);
    assert_eq!(output.summary.error_file_count, 1);
    let reasons = output
        .summary
        .incomplete_reason_counts
        .iter()
        .map(|entry| {
            (
                entry.reason.as_str(),
                (entry.file_count, entry.detail.as_str()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        reasons.get("collector_failure").map(|entry| entry.0),
        Some(1)
    );
    assert_eq!(reasons.get("parser_partial").map(|entry| entry.0), Some(1));
    assert_eq!(output.coverage_gaps.len(), 2);
    let partial = output
        .coverage_gaps
        .iter()
        .find(|entry| entry.reason == FileCoverageReason::ParserPartial)
        .expect("parser partial diagnostic");
    assert!(!partial.retryable);
    assert!(partial.verified_source);
    assert!(partial.projection_available);
}

#[test]
fn build_search_state_ignores_stale_legacy_projection_rows() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(900),
                kind: NodeKind::FILE,
                serialized_name: "src/changed.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(901),
                kind: NodeKind::FUNCTION,
                serialized_name: "old_name".to_string(),
                qualified_name: Some("pkg::old_name".to_string()),
                file_node_id: Some(CoreNodeId(900)),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(910),
                kind: NodeKind::FILE,
                serialized_name: "src/untouched.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(911),
                kind: NodeKind::FUNCTION,
                serialized_name: "untouched".to_string(),
                qualified_name: Some("pkg::untouched".to_string()),
                file_node_id: Some(CoreNodeId(910)),
                ..Default::default()
            },
        ])
        .expect("insert nodes");
    storage
        .insert_nodes_batch(&[Node {
            id: CoreNodeId(901),
            kind: NodeKind::FUNCTION,
            serialized_name: "renamed".to_string(),
            qualified_name: Some("pkg::renamed".to_string()),
            file_node_id: Some(CoreNodeId(900)),
            ..Default::default()
        }])
        .expect("update changed node");
    storage
        .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
            node_id: CoreNodeId(911),
            display_name: "stale_other_file".to_string(),
        }])
        .expect("seed untouched stale projection");

    let nodes = storage.get_nodes().expect("nodes");
    let result = build_search_state(None, nodes).expect("build search state from canonical nodes");
    assert_eq!(
        result.node_names.get(&CoreNodeId(901)).map(String::as_str),
        Some("pkg::renamed")
    );
    assert_eq!(
        result.node_names.get(&CoreNodeId(911)).map(String::as_str),
        Some("pkg::untouched")
    );

    let projection = storage
        .get_search_symbol_projection_batch_after(None, 10)
        .expect("projection");
    let names_by_id: HashMap<_, _> = projection
        .into_iter()
        .map(|entry| (entry.node_id, entry.display_name))
        .collect();
    assert_eq!(
        names_by_id.get(&CoreNodeId(911)).map(String::as_str),
        Some("stale_other_file")
    );
}

#[test]
fn persisted_search_build_streams_multiple_pages_through_one_writer() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("search stream tempdir");
    let storage_path = temp.path().join("codestory.db");
    let search_path = temp.path().join("search-generation");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    let nodes = (1..=4_100_i64)
        .map(|id| Node {
            id: CoreNodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("symbol_{id}"),
            qualified_name: (id % 2 == 0).then(|| format!("pkg::symbol_{id}")),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    storage
        .insert_nodes_batch(&nodes)
        .expect("insert streamed search nodes");

    let cancelled_search_path = temp.path().join("cancelled-search-generation");
    let cancel_token = CancellationToken::new();
    arm_publication_test_fault(
        PublicationTestBoundary::SearchSymbolPage,
        PublicationTestAction::Cancel,
    );
    let cancelled = match build_persisted_search_state_from_canonical_symbols(
        &mut storage,
        &cancelled_search_path,
        false,
        &test_sidecar_runtime_from_env(),
        Some(&cancel_token),
    ) {
        Err(error) => error,
        Ok(_) => panic!("cancel after the first non-final page"),
    };
    assert_eq!(cancelled.code, "cancelled");
    assert!(cancel_token.is_cancelled());
    assert!(
        !search_generation_completion_path(&cancelled_search_path).exists(),
        "cancelled page stream must not publish a completion marker"
    );
    let cancelled_engine = SearchEngine::open_existing(&cancelled_search_path)
        .expect("open uncommitted cancelled generation");
    assert_eq!(
        cancelled_engine.tantivy_doc_count(),
        0,
        "cancelled non-final page must not commit Tantivy documents"
    );
    drop(cancelled_engine);

    let result = build_persisted_search_state_from_canonical_symbols(
        &mut storage,
        &search_path,
        false,
        &test_sidecar_runtime_from_env(),
        None,
    )
    .expect("build persisted search from canonical stream");

    assert_eq!(result.search_stats.search_projection_rebuild_ms, 0);
    assert_eq!(result.search_stats.search_symbol_stream_rows, 4_100);
    assert_eq!(result.search_stats.search_symbol_stream_batches, 2);
    assert_eq!(result.search_stats.search_symbol_index_docs_written, 4_100);
    assert_eq!(result.search_stats.search_symbol_index_writer_count, 1);
    assert_eq!(result.search_stats.search_symbol_index_commit_count, 1);
    assert_eq!(result.search_stats.search_symbol_index_reload_count, 1);
    assert_eq!(result.node_names.len(), 4_100);
    assert_eq!(result.engine.full_text_doc_count(), 4_100);
    assert_eq!(
        storage
            .get_search_symbol_projection_count()
            .expect("count legacy projection"),
        0
    );
}

#[test]
fn search_requires_full_sidecars_for_exact_type_queries() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::FILE,
                    serialized_name: temp
                        .path()
                        .join("src")
                        .join("lib.rs")
                        .to_string_lossy()
                        .to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::STRUCT,
                    serialized_name: "AppController".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(10),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(12),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "AppController::open_project".to_string(),
                    qualified_name: Some("AppController::open_project".to_string()),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(20),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(13),
                    kind: NodeKind::UNKNOWN,
                    serialized_name: "AppController".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(30),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
        .expect("open project");

    let error = controller
        .search(SearchRequest {
            query: "AppController".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn compare_search_hits_prefers_function_over_method_for_equal_symbol_matches() {
    let function = SearchHit {
        node_id: NodeId("function".to_string()),
        display_name: "ArtificialPlayer::min_max".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: None,
        line: None,
        score: 184.0,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    };
    let method = SearchHit {
        node_id: NodeId("method".to_string()),
        display_name: "ArtificialPlayer::min_max".to_string(),
        kind: codestory_contracts::api::NodeKind::METHOD,
        file_path: None,
        line: None,
        score: 184.0,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    };

    let mut hits = [method, function.clone()];
    hits.sort_by(|left, right| compare_search_hits("min_max", left, right));

    assert_eq!(hits.first().map(|hit| hit.kind), Some(function.kind));
}

#[test]
fn search_prefers_full_sidecars_for_tictactoe_queries() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
    let workspace = copy_tictactoe_workspace();
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    controller
        .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open workspace");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("index fixtures");

    for query in ["check_winner", "min_max"] {
        let error = controller
            .search(SearchRequest {
                query: query.to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("search fixtures should require full sidecars");
        assert_mandatory_retrieval_unavailable(&error);
    }
}

#[test]
fn repo_explanation_search_requires_full_sidecar_retrieval() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
    let workspace = copy_tictactoe_workspace();
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    controller
        .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open workspace");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("index fixtures");

    let generic_error = controller
        .search_results(SearchRequest {
            query: "Explain how this repo fits together".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("generic repo explanation search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&generic_error);

    let symbol_error = controller
        .search_results(SearchRequest {
            query: "Explain how check_winner fits in this repo".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: true,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("symbol-like repo explanation search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&symbol_error);
}

#[test]
fn search_rejects_natural_language_queries_without_full_sidecars() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(201),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "language_parsing_pipeline".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(202),
                    kind: NodeKind::MODULE,
                    serialized_name: "parser_core".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(203),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "runtime_workspace_indexer_store_flow".to_string(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let broad_query =
        "Explain how the full-index path flows through runtime workspace indexer and store";
    let error_without_plan = controller
        .search_results(SearchRequest {
            query: broad_query.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 20,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("natural language search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error_without_plan);

    let error_with_plan = controller
        .search_results(SearchRequest {
            query: broad_query.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 20,
            expand_search_plan: true,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("natural language search plan should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error_with_plan);
}

#[test]
fn build_search_state_prefers_qualified_name() {
    let nodes = vec![Node {
        id: CoreNodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "short_name".to_string(),
        qualified_name: Some("pkg.mod.short_name".to_string()),
        ..Default::default()
    }];

    let result = build_search_state(None, nodes).expect("build search state");
    let node_names = result.node_names;
    let engine = result.engine;
    assert_eq!(
        node_names.get(&CoreNodeId(1)).map(String::as_str),
        Some("pkg.mod.short_name")
    );

    let hits = engine.search_symbol("pkg.mod");
    assert_eq!(hits.first().copied(), Some(CoreNodeId(1)));
}

#[test]
fn open_project_summary_clears_search_state() {
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
        .expect("open project with search state");
    assert!(
        controller.state.lock().search_engine.is_some(),
        "expected full open to initialize search state"
    );

    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project summary");
    let state = controller.state.lock();
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn run_indexing_without_runtime_refresh_keeps_search_uninitialized() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let state = controller.state.lock();
    assert!(!state.is_indexing);
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn run_indexing_without_runtime_refresh_populates_dense_anchor_inputs_in_storage() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let state = controller.state.lock();
    assert!(!state.is_indexing);
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
    drop(state);

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let anchors = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs");
    assert!(
        !anchors.is_empty(),
        "expected full indexing to publish dense anchor inputs without requiring a follow-up open"
    );
    assert!(anchors.iter().all(|anchor| {
        !anchor.document_hash.is_empty()
            && anchor.policy_version == SEMANTIC_POLICY_VERSION
            && anchor.source_identity.starts_with("core:")
            && !anchor.text.is_empty()
    }));
    let publication = storage
        .get_complete_index_publication()
        .expect("core publication")
        .expect("complete core publication");
    let manifest = storage
        .validate_dense_anchor_publication(&publication)
        .expect("complete dense anchor manifest");
    assert_eq!(manifest.anchor_count as usize, anchors.len());
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy semantic docs")
            .is_empty(),
        "core indexing must not persist vectors"
    );
}

#[test]
fn unchanged_incremental_refresh_rebinds_the_complete_dense_anchor_generation() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full index");
    let first_storage = Storage::open(&storage_path).expect("first storage");
    let first_publication = first_storage
        .get_complete_index_publication()
        .expect("first publication")
        .expect("complete first publication");
    let first_manifest = first_storage
        .validate_dense_anchor_publication(&first_publication)
        .expect("first dense manifest");
    drop(first_storage);

    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("unchanged incremental index");
    let storage = Storage::open(&storage_path).expect("incremental storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("incremental publication")
        .expect("complete incremental publication");
    let manifest = storage
        .validate_dense_anchor_publication(&publication)
        .expect("incremental dense manifest");
    assert_eq!(manifest.anchor_digest, first_manifest.anchor_digest);
    assert_ne!(manifest.core_run_id, first_manifest.core_run_id);
    let expected_source = format!("core:{}:{}", publication.generation_id, publication.run_id);
    assert!(
        storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("carried dense anchors")
            .iter()
            .all(|anchor| anchor.source_identity == expected_source)
    );
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy docs")
            .is_empty()
    );
}

#[test]
fn core_dense_anchor_publication_ignores_disabled_retrieval_intent() {
    let _lock = process_env_test_lock();
    let _hybrid = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("core index without retrieval activation");

    let storage = Storage::open(&storage_path).expect("core storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("core publication")
        .expect("complete core publication");
    let manifest = storage
        .validate_dense_anchor_publication(&publication)
        .expect("dense anchors are complete without retrieval activation");
    assert!(manifest.anchor_count > 0);
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy docs")
            .is_empty()
    );
    assert!(controller.state.lock().search_engine.is_none());
}

#[test]
fn core_dense_anchor_publication_succeeds_when_embedding_backend_is_unavailable() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let embedding_cache_root = workspace.path().join("embedding-unavailable");
    fs::create_dir_all(&embedding_cache_root).expect("embedding cache root");
    fs::write(
        embedding_cache_root.join(codestory_retrieval::TEST_EMBEDDING_UNAVAILABLE_MARKER),
        b"unavailable",
    )
    .expect("embedding unavailable marker");
    let process_defaults = codestory_retrieval::SidecarProcessDefaults::new(
        embedding_cache_root,
        codestory_retrieval::SidecarRuntimeDefaults::from_process_env(),
    );
    let runtime =
        codestory_retrieval::SidecarRuntimeConfig::for_project_profile_with_process_defaults(
            Some(workspace.path()),
            codestory_retrieval::SidecarProfile::Local,
            None,
            &process_defaults,
            &codestory_retrieval::SidecarRuntimeOverrides::default(),
        );
    let unavailable = codestory_retrieval::ensure_product_embedding_backend_for_runtime(&runtime)
        .expect_err("test runtime must reject embedding initialization");
    assert!(
        unavailable
            .to_string()
            .contains("embedding backend unavailable")
    );
    assert!(!codestory_retrieval::probe_product_embedding_runtime_for_runtime(&runtime).reachable);

    let controller = AppController::new_with_config(runtime);
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("core indexing must not initialize or access embeddings");

    let storage = Storage::open(&storage_path).expect("core storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("core publication")
        .expect("complete core publication");
    let manifest = storage
        .validate_dense_anchor_publication(&publication)
        .expect("dense publication without embeddings");
    assert!(manifest.anchor_count > 0);
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy vectors")
            .is_empty()
    );
}

fn make_source_exceed_default_index_byte_cap(path: &Path, reason: &str) {
    let mut source = fs::read_to_string(path).expect("read source");
    source.push_str("\n// ");
    source.push_str(reason);
    source.push_str("\n// ");
    let padding = (DEFAULT_SOURCE_FILE_BYTE_CAP as usize)
        .saturating_sub(source.len())
        .saturating_add(1);
    source.push_str(&"x".repeat(padding));
    source.push('\n');
    fs::write(path, source).expect("write oversized source");

    let size = fs::metadata(path).expect("oversized source metadata").len();
    assert!(
        size > DEFAULT_SOURCE_FILE_BYTE_CAP,
        "fixture source must exceed the default index byte cap: {size}"
    );
}

#[test]
fn full_refresh_publishes_structural_unit_exclusion_without_graph_claims() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    fs::write(workspace.path().join("small.rs"), "pub fn small() {}\n")
        .expect("write control source");
    let evidence_path = workspace.path().join("evidence-generated.json");
    let mut evidence = String::from("{");
    for index in 0..=codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP {
        if index > 0 {
            evidence.push(',');
        }
        evidence.push_str(&format!("\"key{index}\":{index}"));
    }
    evidence.push('}');
    fs::write(&evidence_path, &evidence).expect("write bounded structural fixture");
    assert!(evidence.len() as u64 <= DEFAULT_SOURCE_FILE_BYTE_CAP);

    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("unit-bound source should not block core publication");

    let storage = Storage::open(&storage_path).expect("open published storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("read publication")
        .expect("complete publication");
    let manifest = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            default_source_policy_identity(),
        )
        .expect("verified exclusion manifest");
    assert_eq!(manifest.exclusion_count, 1);
    let exclusions = storage
        .get_source_policy_exclusions()
        .expect("read exclusions");
    assert_eq!(exclusions[0].normalized_path, "evidence-generated.json");
    assert_eq!(
        exclusions[0].observed_unit_count,
        codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP + 1
    );
    assert_eq!(
        exclusions[0].structural_unit_cap,
        codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP
    );
    assert!(
        storage
            .get_file_by_path(&evidence_path)
            .expect("read file row")
            .is_none(),
        "excluded content must not retain graph or structural projection"
    );
    let files = controller
        .indexed_files(IndexedFilesRequest {
            path_contains: Some("evidence-generated.json".into()),
            language: None,
            role: None,
            limit: None,
        })
        .expect("read native exclusion diagnostics");
    assert_eq!(files.policy_exclusions.len(), 1);
    assert!(!files.policy_exclusions[0].graph_coverage);
    assert!(!files.policy_exclusions[0].semantic_coverage);
    assert_eq!(
        files.policy_exclusions[0].observed_unit_count,
        codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP + 1
    );
    let workspace_manifest =
        WorkspaceManifest::open(workspace.path().to_path_buf()).expect("workspace manifest");
    let freshness = index_freshness_from_storage(workspace.path(), &workspace_manifest, &storage);
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(freshness.changed_file_count, 0);
    assert_eq!(freshness.new_file_count, 0);
    assert_eq!(freshness.removed_file_count, 0);
}

#[test]
fn incremental_refresh_replaces_structural_projection_and_semantics_with_unit_exclusion() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let component_dir = workspace.path().join("alpha");
    fs::create_dir_all(&component_dir).expect("component directory");
    let evidence_path = component_dir.join("evidence.json");
    fs::write(&evidence_path, "{\"kept\":1}").expect("initial structural source");
    fs::write(workspace.path().join("control.rs"), "pub fn control() {}\n")
        .expect("control source");

    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full publication");

    let initial_storage = Storage::open(&storage_path).expect("initial storage");
    let initial_file = initial_storage
        .get_file_by_path(&evidence_path)
        .expect("initial file lookup")
        .expect("initial parser-backed file");
    let initial_file_id = codestory_contracts::graph::NodeId(initial_file.id);
    assert!(
        initial_storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("initial symbol docs")
            .iter()
            .any(|doc| doc.file_node_id == Some(initial_file_id)),
        "the initial structural projection must have semantic evidence to invalidate"
    );
    assert!(
        initial_storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("initial component reports")
            .iter()
            .any(|doc| doc.display_name == "component_report:dir:alpha"),
        "the initial structural projection must contribute an alpha component report"
    );
    drop(initial_storage);

    let mut over_bound = String::from("{");
    for index in 0..=codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP {
        if index > 0 {
            over_bound.push(',');
        }
        over_bound.push_str(&format!("\"key{index}\":{index}"));
    }
    over_bound.push('}');
    assert!(over_bound.len() as u64 <= DEFAULT_SOURCE_FILE_BYTE_CAP);
    fs::write(&evidence_path, over_bound).expect("unit-bound structural source");

    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental unit exclusion publication");
    let excluded_storage = Storage::open(&storage_path).expect("excluded storage");
    assert!(
        excluded_storage
            .get_file_by_path(&evidence_path)
            .expect("excluded file lookup")
            .is_none(),
        "unit exclusion must remove the previous parser-backed projection"
    );
    assert!(
        excluded_storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("excluded symbol docs")
            .iter()
            .all(|doc| {
                doc.file_node_id != Some(initial_file_id)
                    && doc.display_name != "component_report:dir:alpha"
            }),
        "unit exclusion must remove stale file semantics and its component report"
    );
    assert!(
        excluded_storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("excluded dense anchors")
            .iter()
            .all(|doc| doc.file_node_id != Some(initial_file_id)),
        "unit exclusion must remove stale dense evidence"
    );
    assert_eq!(
        excluded_storage
            .get_source_policy_exclusions()
            .expect("unit exclusions")
            .len(),
        1
    );
    drop(excluded_storage);

    fs::write(&evidence_path, "{\"reevaluated\":2}").expect("source changed back below unit cap");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental policy reevaluation");
    let restored_storage = Storage::open(&storage_path).expect("restored storage");
    assert!(
        restored_storage
            .get_file_by_path(&evidence_path)
            .expect("restored file lookup")
            .is_some(),
        "changed content below the policy cap must be indexed again"
    );
    assert!(
        restored_storage
            .get_source_policy_exclusions()
            .expect("restored exclusions")
            .is_empty(),
        "the old content-bound exclusion must not survive reevaluation"
    );
}

#[test]
fn incremental_structural_unit_exclusion_revalidates_content_at_identity_fence() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let evidence_path = workspace.path().join("evidence.json");
    fs::write(&evidence_path, "{\"baseline\":1}").expect("baseline structural source");
    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("baseline publication");
    let baseline = Storage::open(&storage_path)
        .expect("baseline storage")
        .get_complete_index_publication()
        .expect("baseline publication read")
        .expect("complete baseline publication");

    let mut over_bound = String::from("{");
    for index in 0..=codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP {
        if index > 0 {
            over_bound.push(',');
        }
        over_bound.push_str(&format!("\"key{index}\":{index}"));
    }
    over_bound.push('}');
    fs::write(&evidence_path, over_bound).expect("unit-bound structural source");
    let changed_path = evidence_path.clone();
    arm_source_policy_before_revalidate_hook(move || {
        let mut bytes = fs::read(&changed_path).expect("classified unit exclusion");
        bytes.push(b' ');
        fs::write(&changed_path, bytes).expect("drift classified unit exclusion");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect_err("drifted unit exclusion must fail closed");
    assert_eq!(error.code, "source_verification_failed");
    let live = Storage::open(&storage_path).expect("preserved live storage");
    assert_eq!(
        live.get_complete_index_publication()
            .expect("preserved publication"),
        Some(baseline)
    );
    assert!(
        live.get_file_by_path(&evidence_path)
            .expect("preserved parser-backed file")
            .is_some()
    );
    assert!(
        live.get_source_policy_exclusions()
            .expect("preserved exclusions")
            .is_empty()
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn structural_unit_policy_change_invalidates_exclusion_and_forces_reevaluation() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let evidence_path = workspace.path().join("evidence.json");
    fs::write(&evidence_path, "{\"one\":1,\"two\":2,\"three\":3}").expect("structural source");
    fs::write(workspace.path().join("control.rs"), "pub fn control() {}\n")
        .expect("control source");
    let storage_path = workspace.path().join(".cache/codestory.db");
    let excluding_policy = SourceIndexPolicy {
        policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
        byte_cap: DEFAULT_SOURCE_FILE_BYTE_CAP,
        structural_unit_cap: 2,
    };
    let excluding_controller = AppController::new_with_source_index_policy(
        test_sidecar_runtime_from_env(),
        excluding_policy.clone(),
    );
    excluding_controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open excluding controller");
    excluding_controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish custom-cap exclusion");

    let excluded_storage = Storage::open(&storage_path).expect("excluded storage");
    let publication = excluded_storage
        .get_complete_index_publication()
        .expect("excluded publication")
        .expect("complete excluded publication");
    let excluded_manifest = excluded_storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &excluding_policy.policy_version,
                excluding_policy.byte_cap,
                excluding_policy.structural_unit_cap,
            ),
        )
        .expect("custom unit cap manifest");
    assert_eq!(excluded_manifest.structural_unit_cap, 2);
    assert_eq!(excluded_manifest.exclusion_count, 1);
    drop(excluded_storage);

    let admitting_policy = SourceIndexPolicy {
        structural_unit_cap: 3,
        ..excluding_policy
    };
    let admitting_controller = AppController::new_with_source_index_policy(
        test_sidecar_runtime_from_env(),
        admitting_policy.clone(),
    );
    admitting_controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open admitting controller");
    assert!(
        admitting_controller
            .complete_core_requires_publication_repair(&storage_path)
            .expect("policy change freshness"),
        "a changed unit cap must invalidate the prior publication identity"
    );
    let error = admitting_controller
        .indexed_files(IndexedFilesRequest {
            path_contains: None,
            language: None,
            role: None,
            limit: None,
        })
        .expect_err("mismatched unit-cap reader must fail closed");
    assert_eq!(error.code, "source_verification_failed");

    admitting_controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("reevaluate source under changed unit cap");
    let admitted_storage = Storage::open(&storage_path).expect("admitted storage");
    let admitted_publication = admitted_storage
        .get_complete_index_publication()
        .expect("admitted publication")
        .expect("complete admitted publication");
    let admitted_manifest = admitted_storage
        .validate_source_policy_exclusion_publication(
            &admitted_publication,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &admitting_policy.policy_version,
                admitting_policy.byte_cap,
                admitting_policy.structural_unit_cap,
            ),
        )
        .expect("changed-cap manifest");
    assert_eq!(admitted_manifest.structural_unit_cap, 3);
    assert_eq!(admitted_manifest.exclusion_count, 0);
    assert!(
        admitted_storage
            .get_file_by_path(&evidence_path)
            .expect("reevaluated file")
            .is_some()
    );
}

#[test]
fn first_full_refresh_publishes_verified_oversized_exclusion_without_graph_coverage() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    make_source_exceed_default_index_byte_cap(
        &workspace.path().join("rust_tictactoe.rs"),
        "first full-refresh candidate is deliberately oversized",
    );
    fs::create_dir_all(workspace.path().join("generated")).expect("generated fixture dir");
    fs::create_dir_all(workspace.path().join("vendor")).expect("vendor fixture dir");
    fs::write(
        workspace.path().join("generated/registers.h"),
        vec![b'g'; DEFAULT_SOURCE_FILE_BYTE_CAP as usize + 1],
    )
    .expect("generated oversized fixture");
    fs::write(
        workspace.path().join("vendor/bundle.js"),
        vec![b'v'; DEFAULT_SOURCE_FILE_BYTE_CAP as usize + 1],
    )
    .expect("vendor oversized fixture");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("verified oversized source should not block first publication");

    let storage = Storage::open(&storage_path).expect("open published storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("read publication")
        .expect("complete publication");
    let manifest = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            default_source_policy_identity(),
        )
        .expect("verified exclusion manifest");
    assert_eq!(manifest.exclusion_count, 3);
    let exclusions = storage
        .get_source_policy_exclusions()
        .expect("source exclusions");
    assert_eq!(
        exclusions
            .iter()
            .map(|entry| entry.normalized_path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "generated/registers.h",
            "rust_tictactoe.rs",
            "vendor/bundle.js"
        ]
    );
    assert!(
        exclusions
            .iter()
            .all(|entry| entry.observed_size > entry.byte_cap)
    );
    assert!(
        storage
            .get_file_by_path(&workspace.path().join("rust_tictactoe.rs"))
            .expect("excluded file lookup")
            .is_none(),
        "policy exclusion must not create parser-backed file coverage"
    );
    let files = controller
        .indexed_files(IndexedFilesRequest {
            path_contains: Some("rust_tictactoe.rs".into()),
            language: None,
            role: None,
            limit: None,
        })
        .expect("agent-facing file coverage");
    assert!(files.coverage_gaps.is_empty());
    assert_eq!(files.policy_exclusions.len(), 1);
    assert!(!files.policy_exclusions[0].graph_coverage);
    assert!(!files.policy_exclusions[0].semantic_coverage);
    let all_files = controller
        .indexed_files(IndexedFilesRequest {
            path_contains: None,
            language: None,
            role: None,
            limit: None,
        })
        .expect("all agent-facing file coverage");
    assert_eq!(all_files.summary.policy_exclusion_count, 3);
    assert!(
        all_files
            .policy_exclusions
            .iter()
            .any(|entry| entry.role == IndexedFileRoleDto::Generated)
    );
    assert!(
        all_files
            .policy_exclusions
            .iter()
            .any(|entry| entry.role == IndexedFileRoleDto::Vendor)
    );
    let workspace_manifest =
        WorkspaceManifest::open(workspace.path().to_path_buf()).expect("workspace manifest");
    let freshness = index_freshness_from_storage(workspace.path(), &workspace_manifest, &storage);
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    storage
        .get_connection()
        .execute("DELETE FROM source_policy_exclusion_publication", [])
        .expect("corrupt exclusion publication identity");
    assert!(
        controller
            .complete_core_requires_publication_repair(&storage_path)
            .expect("missing migrated manifest requires writer repair")
    );
    let incomplete = index_freshness_from_storage(workspace.path(), &workspace_manifest, &storage);
    assert_eq!(incomplete.status, IndexFreshnessStatusDto::NotChecked);
    assert!(
        incomplete
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("source policy exclusion publication"))
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn full_refresh_revalidates_excluded_bytes_at_identity_fence_and_preserves_live_core() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache/codestory.db");
    let source_path = workspace.path().join("rust_tictactoe.rs");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline core");
    let baseline = Storage::open(&storage_path)
        .expect("open baseline core")
        .get_complete_index_publication()
        .expect("read baseline publication")
        .expect("baseline publication");
    make_source_exceed_default_index_byte_cap(
        &source_path,
        "full refresh identity-fence drift fixture",
    );
    let changed_path = source_path.clone();
    arm_source_policy_before_revalidate_hook(move || {
        let mut bytes = fs::read(&changed_path).expect("read classified exclusion");
        bytes.extend_from_slice(b"\n// changed after classification\n");
        fs::write(&changed_path, bytes).expect("mutate classified exclusion");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("changed exclusion must reject full candidate");
    assert_eq!(error.code, "source_verification_failed");
    let live = Storage::open(&storage_path).expect("reopen preserved live core");
    assert_eq!(
        live.get_complete_index_publication()
            .expect("read preserved publication"),
        Some(baseline.clone())
    );
    let manifest = live
        .validate_source_policy_exclusion_publication(
            &baseline,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            default_source_policy_identity(),
        )
        .expect("baseline exclusion manifest remains valid");
    assert_eq!(manifest.exclusion_count, 0);
    assert!(
        live.get_file_by_path(&source_path)
            .expect("read preserved file projection")
            .is_some()
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn incremental_refresh_revalidates_excluded_bytes_at_identity_fence_and_preserves_live_core() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache/codestory.db");
    let source_path = workspace.path().join("rust_tictactoe.rs");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline core");
    let baseline = Storage::open(&storage_path)
        .expect("open baseline core")
        .get_complete_index_publication()
        .expect("read baseline publication")
        .expect("baseline publication");
    make_source_exceed_default_index_byte_cap(
        &source_path,
        "incremental identity-fence drift fixture",
    );
    let changed_path = source_path.clone();
    arm_source_policy_before_revalidate_hook(move || {
        let mut bytes = fs::read(&changed_path).expect("read classified exclusion");
        bytes.extend_from_slice(b"\n// changed after classification\n");
        fs::write(&changed_path, bytes).expect("mutate classified exclusion");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect_err("changed exclusion must reject incremental candidate");
    assert_eq!(error.code, "source_verification_failed");
    let live = Storage::open(&storage_path).expect("reopen preserved live core");
    assert_eq!(
        live.get_complete_index_publication()
            .expect("read preserved publication"),
        Some(baseline.clone())
    );
    let manifest = live
        .validate_source_policy_exclusion_publication(
            &baseline,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            default_source_policy_identity(),
        )
        .expect("baseline exclusion manifest remains valid");
    assert_eq!(manifest.exclusion_count, 0);
    assert!(
        live.get_file_by_path(&source_path)
            .expect("read preserved file projection")
            .is_some()
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn non_default_source_policy_cap_is_shared_by_planning_indexer_publication_and_readers() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let large_path = workspace.path().join("large.rs");
    fs::write(workspace.path().join("small.rs"), "pub fn small() {}\n")
        .expect("write small source");
    fs::write(
        &large_path,
        "// oversized\n// xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n",
    )
    .expect("write policy source");
    let policy = SourceIndexPolicy::oversized(64);
    let manifest = WorkspaceManifest::open(workspace.path().to_path_buf()).expect("workspace");
    let inventory = manifest
        .source_inventory_with_policy(&policy)
        .expect("classify with explicit policy");
    assert_eq!(inventory.policy_exclusions.len(), 1);
    assert_eq!(inventory.policy_exclusions[0].normalized_path, "large.rs");
    assert_eq!(inventory.policy_exclusions[0].byte_cap, 64);

    let mut fallback = Storage::new_in_memory().expect("fallback storage");
    let fallback_plan = RefreshExecutionPlan {
        mode: RefreshMode::FullRefresh,
        files_to_index: vec![large_path.clone()],
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    V2WorkspaceIndexer::new(workspace.path().to_path_buf())
        .with_source_file_byte_cap(policy.byte_cap)
        .run(&mut fallback, &fallback_plan, &EventBus::new(), None)
        .expect("indexer fallback records oversized coverage");
    assert!(
        fallback
            .get_errors(None)
            .expect("fallback errors")
            .iter()
            .any(|error| error.coverage_reason == Some(FileCoverageReason::Oversized)),
        "the parser fallback must enforce the same 64-byte cap"
    );

    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new_with_source_index_policy(
        test_sidecar_runtime_from_env(),
        policy.clone(),
    );
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish non-default policy core");
    let storage = Storage::open(&storage_path).expect("open policy core");
    let publication = storage
        .get_complete_index_publication()
        .expect("read policy publication")
        .expect("complete policy publication");
    let published = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &project_identity_v3(workspace.path()).project_id,
            &project_identity_v3(workspace.path()).workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &policy.policy_version,
                policy.byte_cap,
                policy.structural_unit_cap,
            ),
        )
        .expect("manifest uses the injected policy");
    assert_eq!(published.byte_cap, 64);
    assert_eq!(published.exclusion_count, 1);
    let files = controller
        .indexed_files(IndexedFilesRequest {
            path_contains: Some("large.rs".into()),
            language: None,
            role: None,
            limit: None,
        })
        .expect("matching policy reader accepts the manifest");
    assert_eq!(files.policy_exclusions[0].byte_cap, 64);

    for incompatible in [
        SourceIndexPolicy::oversized(65),
        SourceIndexPolicy {
            policy_version: "oversized-source-v2".into(),
            byte_cap: 64,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        },
    ] {
        let reader = AppController::new_with_source_index_policy(
            test_sidecar_runtime_from_env(),
            incompatible,
        );
        reader
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("bind incompatible reader");
        assert!(
            reader
                .complete_core_requires_publication_repair(&storage_path)
                .expect("inspect repair requirement")
        );
        let error = reader
            .indexed_files(IndexedFilesRequest {
                path_contains: None,
                language: None,
                role: None,
                limit: None,
            })
            .expect_err("incompatible reader must fail closed");
        assert_eq!(error.code, "source_verification_failed");
    }
}

#[test]
fn special_collector_growth_after_planning_cannot_publish() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let source_path = workspace.path().join("schema.sql");
    fs::write(&source_path, "CREATE TABLE drifted (id INTEGER);\n")
        .expect("write below-cap structural source");
    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new_with_source_index_policy(
        test_sidecar_runtime_from_env(),
        SourceIndexPolicy::oversized(64),
    );
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");

    let changed_path = source_path.clone();
    arm_source_policy_after_plan_hook(move || {
        let mut bytes = fs::read(&changed_path).expect("read planned structural source");
        bytes.resize(65, b' ');
        fs::write(&changed_path, bytes).expect("grow structural source after planning");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("post-plan oversized structural source must reject publication");
    assert_eq!(error.code, "source_oversized");
    assert!(error.details.as_ref().is_some_and(|details| {
        details.coverage_gaps.iter().any(|gap| {
            gap.path == "schema.sql"
                && gap.reason == FileCoverageReason::Oversized
                && !gap.verified_source
                && !gap.projection_available
        })
    }));
    if storage_path.exists() {
        assert!(
            Storage::open(&storage_path)
                .expect("open rejected live storage")
                .get_index_publication()
                .expect("read rejected publication")
                .is_none()
        );
    }
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn partial_discovery_keeps_oversized_candidates_blocking_and_publishes_nothing() {
    let workspace = tempdir().expect("workspace dir");
    fs::create_dir_all(workspace.path().join("src")).expect("source directory");
    fs::write(
        workspace.path().join("src/large.rs"),
        vec![b'x'; DEFAULT_SOURCE_FILE_BYTE_CAP as usize + 1],
    )
    .expect("oversized source");
    fs::write(
        workspace.path().join("codestory_workspace.json"),
        r#"{"members":["src","missing"]}"#,
    )
    .expect("partial workspace manifest");
    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open partial project");
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("partial discovery cannot authorize exclusions");

    assert_eq!(error.code, "source_discovery_incomplete");
    assert!(error.details.as_ref().is_some_and(|details| {
        details
            .coverage_gaps
            .iter()
            .any(|gap| gap.reason == FileCoverageReason::DiscoveryIncomplete)
    }));
    if storage_path.exists() {
        let storage = Storage::open(&storage_path).expect("partial storage");
        assert!(
            storage
                .get_source_policy_exclusion_manifest()
                .expect("partial exclusion manifest")
                .is_none()
        );
        assert!(
            storage
                .get_index_publication()
                .expect("partial core publication")
                .is_none()
        );
    }
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn changed_source_is_reevaluated_into_a_new_verified_exclusion() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let source_path = workspace.path().join("rust_tictactoe.rs");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let first_storage = Storage::open(&storage_path).expect("first storage");
    let first_publication = first_storage
        .get_complete_index_publication()
        .expect("first publication")
        .expect("complete first publication");
    let first_file_id = first_storage
        .get_file_by_path(&source_path)
        .expect("first file lookup")
        .expect("indexed Rust source")
        .id;
    let first_anchors = first_storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("first anchors")
        .into_iter()
        .filter(|anchor| anchor.file_node_id == Some(CoreNodeId(first_file_id)))
        .map(|anchor| (anchor.node_id, anchor.document_hash, anchor.text))
        .collect::<HashSet<_>>();
    assert!(
        !first_anchors.is_empty(),
        "fixture needs Rust dense anchors"
    );
    drop(first_storage);

    make_source_exceed_default_index_byte_cap(
        &source_path,
        "scheduled but deliberately oversized for this index run",
    );
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental exclusion publication");

    let storage = Storage::open(&storage_path).expect("incremental storage");
    assert!(
        storage
            .get_file_by_path(&source_path)
            .expect("file lookup")
            .is_none(),
        "excluded source must not retain parser-backed file coverage"
    );
    let publication = storage
        .get_complete_index_publication()
        .expect("incremental publication")
        .expect("complete incremental publication");
    assert_ne!(publication, first_publication);
    let identity = project_identity_v3(workspace.path());
    storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &identity.project_id,
            &identity.workspace_id,
            default_source_policy_identity(),
        )
        .expect("complete exclusion publication");
    let first_exclusion = storage
        .get_source_policy_exclusions()
        .expect("first exclusions")
        .into_iter()
        .find(|entry| entry.normalized_path == "rust_tictactoe.rs")
        .expect("Rust exclusion");
    let retained_anchors = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("current anchors")
        .into_iter()
        .filter(|anchor| anchor.file_node_id == Some(CoreNodeId(first_file_id)))
        .map(|anchor| (anchor.node_id, anchor.document_hash, anchor.text))
        .collect::<HashSet<_>>();
    assert!(retained_anchors.is_empty());
    assert!(!first_anchors.is_empty());
    drop(storage);

    fs::write(
        &source_path,
        format!(
            "{}\n// changed oversized content\n",
            fs::read_to_string(&source_path).unwrap()
        ),
    )
    .expect("change oversized source");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("changed exclusion reevaluation");
    let changed = Storage::open(&storage_path)
        .expect("changed storage")
        .get_source_policy_exclusions()
        .expect("changed exclusions")
        .into_iter()
        .find(|entry| entry.normalized_path == "rust_tictactoe.rs")
        .expect("changed Rust exclusion");
    assert_ne!(changed.content_hash, first_exclusion.content_hash);
    assert!(changed.observed_size > first_exclusion.observed_size);
}

#[test]
fn semantic_projection_republish_uses_stored_core_after_source_is_removed() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let identity = project_identity_v3(workspace.path());
    let mut before_storage = Storage::open(&storage_path).expect("open complete core");
    let before = before_storage
        .get_complete_index_publication()
        .expect("read complete core")
        .expect("complete publication");
    let exclusions = (0_u64..112)
        .map(|index| OversizedSourceExclusionCandidate {
            normalized_path: format!("legacy/excluded-{index}.rs"),
            content_hash: format!("{index:064x}"),
            observed_size: DEFAULT_SOURCE_FILE_BYTE_CAP + 1 + index,
            observed_unit_count: 0,
            policy_version: LEGACY_OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
            byte_cap: DEFAULT_SOURCE_FILE_BYTE_CAP,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        })
        .collect::<Vec<_>>();
    before_storage
        .publish_source_policy_exclusion_generation(
            &before,
            &identity.project_id,
            &identity.workspace_id,
            legacy_source_policy_identity(),
            &exclusions,
        )
        .expect("replace retained source-policy publication");
    let legacy_source_policy_digest = legacy_source_policy_exclusion_digest_for_test(
        &before_storage
            .get_source_policy_exclusions()
            .expect("read retained source-policy exclusions"),
    );
    let dense_before = before_storage
        .validate_dense_anchor_publication(&before)
        .expect("retained dense publication");
    assert!(dense_before.anchor_count > 0);
    let symbol_doc_count = before_storage
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("retained symbol documents")
        .len();
    assert!(symbol_doc_count > 0);
    before_storage
        .upsert_retrieval_index_manifest(&test_retrieval_manifest(
            &identity.project_id,
            symbol_doc_count as i64,
            dense_before.anchor_count as i64,
        ))
        .expect("publish retained retrieval manifest");
    let before_retrieval = before_storage
        .get_retrieval_index_publication(&identity.project_id)
        .expect("read retrieval publication")
        .expect("retained retrieval publication");
    drop(before_storage);

    let legacy = rusqlite::Connection::open(&storage_path).expect("open retained v1 core");
    legacy
        .execute(
            "UPDATE source_policy_exclusion_publication
             SET schema_version = 1, exclusion_digest = ?1",
            rusqlite::params![legacy_source_policy_digest],
        )
        .expect("restore authentic retained v1 publication identity");
    legacy
        .execute_batch(
            "DELETE FROM structural_text_unit_publication;
             ALTER TABLE index_publication RENAME TO index_publication_v30;
             CREATE TABLE index_publication (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                generation INTEGER NOT NULL CHECK (generation > 0),
                generation_id TEXT NOT NULL UNIQUE CHECK (length(generation_id) > 0),
                run_id TEXT NOT NULL CHECK (length(run_id) > 0),
                mode TEXT NOT NULL CHECK (mode IN ('full', 'incremental')),
                published_at_epoch_ms INTEGER NOT NULL CHECK (published_at_epoch_ms >= 0)
             );
             INSERT INTO index_publication
             SELECT * FROM index_publication_v30;
             DROP TABLE index_publication_v30;
             PRAGMA user_version = 29;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .expect("downgrade retained core to schema 29");
    drop(legacy);
    for entry in fs::read_dir(workspace.path()).expect("list fixture root") {
        let path = entry.expect("fixture entry").path();
        if path.file_name().is_some_and(|name| name == ".cache") {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(&path).expect("remove source directory");
        } else {
            fs::remove_file(&path).expect("remove source file");
        }
    }

    let outcome = controller
        .republish_semantic_projections_at_blocking(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("republish from stored core");

    assert_eq!(outcome.previous_publication, before);
    assert_eq!(outcome.publication.generation, before.generation + 1);
    assert_eq!(
        outcome.publication.mode,
        IndexPublicationMode::SemanticProjection
    );
    assert!(
        outcome
            .phase_timings
            .symbol_search_docs_written
            .is_some_and(|count| count > 0)
    );
    let storage = Storage::open(&storage_path).expect("open republished core");
    assert_eq!(
        storage
            .get_connection()
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("schema version"),
        30
    );
    assert_eq!(
        storage
            .get_complete_index_publication()
            .expect("read republished core"),
        Some(outcome.publication.clone())
    );
    storage
        .validate_dense_anchor_publication(&outcome.publication)
        .expect("dense publication is coherent");
    storage
        .validate_structural_text_unit_publication(&outcome.publication)
        .expect("structural publication is rebound");
    let structural = storage
        .get_structural_text_unit_publication_manifest()
        .expect("read structural manifest")
        .expect("explicit empty structural manifest");
    assert_eq!(structural.unit_count, 0);
    assert_eq!(structural.projection_count, 0);
    let source_manifest = storage
        .validate_source_policy_exclusion_publication(
            &outcome.publication,
            &identity.project_id,
            &identity.workspace_id,
            default_source_policy_identity(),
        )
        .expect("source policy is rebound");
    assert_eq!(source_manifest.exclusion_count, 112);
    assert_eq!(
        storage
            .get_retrieval_index_publication(&identity.project_id)
            .expect("read unchanged retrieval publication")
            .as_ref(),
        Some(&before_retrieval),
        "projection-only core publication must not synthesize retrieval artifacts"
    );
    drop(storage);
    let incompatible = AppController::new_with_source_index_policy(
        test_sidecar_runtime_from_env(),
        SourceIndexPolicy::oversized(DEFAULT_SOURCE_FILE_BYTE_CAP + 1),
    );
    incompatible
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open core with incompatible source policy");
    let error = incompatible
        .republish_semantic_projections_blocking()
        .expect_err("source policy drift must fail closed");
    assert_eq!(error.code, "semantic_projection_migration_required");
    assert_eq!(
        Storage::database_complete_index_publication(&storage_path)
            .expect("read publication after rejected source policy"),
        Some(outcome.publication)
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn semantic_projection_republish_fails_closed_when_stored_document_is_missing() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let before = {
        let mut storage = Storage::open(&storage_path).expect("open complete core");
        let publication = storage
            .get_complete_index_publication()
            .expect("read complete core")
            .expect("complete publication");
        assert!(
            storage
                .clear_symbol_search_docs()
                .expect("remove stored semantic documents")
                > 0
        );
        publication
    };

    let error = controller
        .republish_semantic_projections_blocking()
        .expect_err("missing stored document must fail closed");
    assert_eq!(error.code, "semantic_projection_migration_required");
    assert_eq!(
        Storage::database_complete_index_publication(&storage_path)
            .expect("read preserved publication"),
        Some(before)
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn semantic_projection_republish_rejects_a_cache_owned_by_another_project() {
    let _env = hybrid_test_env();
    let selected = copy_tictactoe_workspace();
    let owner = copy_tictactoe_workspace();
    let storage_path = owner.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(owner.path().to_path_buf(), storage_path.clone())
        .expect("open cache owner");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish owned core");
    let before = Storage::database_complete_index_publication(&storage_path)
        .expect("read owned publication");
    let search_before = persisted_search_generation_names(&storage_path);

    let error = controller
        .republish_semantic_projections_at_blocking(
            selected.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect_err("foreign cache must fail closed");

    assert_eq!(error.code, "semantic_projection_project_mismatch");
    assert_eq!(
        Storage::database_complete_index_publication(&storage_path)
            .expect("read preserved owned publication"),
        before
    );
    assert_eq!(
        persisted_search_generation_names(&storage_path),
        search_before
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn semantic_projection_republish_rejects_manifestless_nonempty_structural_state() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let before = Storage::database_complete_index_publication(&storage_path)
        .expect("read complete publication");
    {
        let storage = Storage::open(&storage_path).expect("open structural fixture");
        storage
            .get_connection()
            .execute_batch(
                "DELETE FROM structural_text_unit_publication;
                 INSERT INTO structural_text_artifact_cache (
                    file_path, file_id, cache_key, source_content_hash,
                    descriptor_version, producer, artifact_digest, artifact_blob,
                    updated_at_epoch_ms
                 ) VALUES ('legacy.txt', -1, 'v1:test',
                    '1111111111111111111111111111111111111111111111111111111111111111',
                    1, 'test',
                    '2222222222222222222222222222222222222222222222222222222222222222',
                    X'01', 1);",
            )
            .expect("seed nonempty unmanifested structural state");
    }

    let error = controller
        .republish_semantic_projections_blocking()
        .expect_err("unmanifested structural rows must fail closed");

    assert_eq!(error.code, "semantic_projection_migration_required");
    assert!(error.message.contains("nonempty state"));
    assert_eq!(
        Storage::database_complete_index_publication(&storage_path)
            .expect("read preserved publication"),
        before
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn semantic_projection_republish_rejects_manifestless_current_schema() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let before = Storage::database_complete_index_publication(&storage_path)
        .expect("read complete publication");
    Storage::open(&storage_path)
        .expect("open current core")
        .get_connection()
        .execute("DELETE FROM structural_text_unit_publication", [])
        .expect("remove current structural manifest");

    let error = controller
        .republish_semantic_projections_blocking()
        .expect_err("current schema cannot use legacy compatibility");

    assert_eq!(error.code, "semantic_projection_migration_required");
    assert!(error.message.contains("schema-29 retained core"));
    assert_eq!(
        Storage::database_complete_index_publication(&storage_path)
            .expect("read preserved publication"),
        before
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn semantic_projection_republish_respects_the_shared_writer_lock() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let publication = Storage::database_complete_index_publication(&storage_path)
        .expect("read complete publication");
    let _guard = IndexWriterGuard::try_acquire(&storage_path).expect("hold writer lock");

    let error = controller
        .republish_semantic_projections_blocking()
        .expect_err("second writer must be rejected");
    assert_eq!(error.code, "cache_busy");
    assert_eq!(
        Storage::database_complete_index_publication(&storage_path)
            .expect("read unchanged publication"),
        publication
    );
}

#[test]
fn semantic_projection_republish_fail_and_cancel_matrix_preserves_complete_core_and_search() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let runtime = test_sidecar_runtime_from_env();
    let controller = AppController::new_with_config(runtime.clone());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let identity = project_identity_v3(workspace.path());
    let (publication, retrieval_publication) = {
        let mut storage = Storage::open(&storage_path).expect("open baseline core");
        let publication = storage
            .get_complete_index_publication()
            .expect("read baseline publication")
            .expect("complete baseline publication");
        let symbol_doc_count = storage
            .get_symbol_search_doc_count()
            .expect("count baseline symbol documents");
        let dense_count = storage
            .validate_dense_anchor_publication(&publication)
            .expect("validate baseline dense publication")
            .anchor_count;
        storage
            .upsert_retrieval_index_manifest(&test_retrieval_manifest(
                &identity.project_id,
                symbol_doc_count as i64,
                dense_count as i64,
            ))
            .expect("publish baseline retrieval identity");
        let retrieval = storage
            .get_retrieval_index_publication(&identity.project_id)
            .expect("read baseline retrieval identity")
            .expect("baseline retrieval publication");
        (publication, retrieval)
    };
    let search_generations = persisted_search_generation_names(&storage_path);
    for boundary in [
        PublicationTestBoundary::SemanticContextIndexes,
        PublicationTestBoundary::SemanticNodePage,
        PublicationTestBoundary::SemanticStoredDocumentPage,
        PublicationTestBoundary::SemanticEndpointRead,
        PublicationTestBoundary::ProjectionSnapshotFinalize,
        PublicationTestBoundary::ProjectionSnapshotDetail,
        PublicationTestBoundary::ProjectionManifestIdentity,
        PublicationTestBoundary::SearchBuild,
        PublicationTestBoundary::SearchSymbolPage,
        PublicationTestBoundary::SearchIndexWrite,
        PublicationTestBoundary::SearchValidation,
        PublicationTestBoundary::SearchCompletion,
        PublicationTestBoundary::CatalogLock,
        PublicationTestBoundary::MarkerCompletion,
        PublicationTestBoundary::DatabaseReplacement,
    ] {
        for action in [PublicationTestAction::Fail, PublicationTestAction::Cancel] {
            let cancel = CancellationToken::new();
            arm_publication_test_fault(boundary, action);
            let error = match semantic_projection_republish_for_runtime(
                workspace.path(),
                &storage_path,
                Some(&cancel),
                &runtime,
                controller.source_index_policy.as_ref(),
            ) {
                Err(error) => error,
                Ok(_) => panic!("faulted projection republish must not publish"),
            };
            assert_eq!(
                error.code,
                if action == PublicationTestAction::Cancel {
                    "cancelled"
                } else {
                    "internal"
                },
                "boundary={boundary:?} action={action:?}: {error:?}"
            );
            assert_eq!(
                cancel.is_cancelled(),
                action == PublicationTestAction::Cancel
            );
            assert_eq!(
                Storage::database_complete_index_publication(&storage_path)
                    .expect("read preserved publication"),
                Some(publication.clone()),
                "boundary={boundary:?} action={action:?}"
            );
            assert_eq!(
                persisted_search_generation_names(&storage_path),
                search_generations,
                "boundary={boundary:?} action={action:?}"
            );
            assert_eq!(
                Storage::open(&storage_path)
                    .expect("open preserved retrieval state")
                    .get_retrieval_index_publication(&identity.project_id)
                    .expect("read preserved retrieval state"),
                Some(retrieval_publication.clone()),
                "boundary={boundary:?} action={action:?}"
            );
            assert_no_staged_publication_artifacts(&storage_path);
        }
    }
}

#[test]
fn semantic_projection_republish_runtime_cache_fault_completes_committed_generation() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let identity = project_identity_v3(workspace.path());
    let (mut previous_publication, retrieval_publication) = {
        let mut storage = Storage::open(&storage_path).expect("open baseline core");
        let publication = storage
            .get_complete_index_publication()
            .expect("read baseline publication")
            .expect("complete baseline publication");
        let symbol_doc_count = storage
            .get_symbol_search_doc_count()
            .expect("count baseline symbol documents");
        let dense_count = storage
            .validate_dense_anchor_publication(&publication)
            .expect("validate baseline dense publication")
            .anchor_count;
        storage
            .upsert_retrieval_index_manifest(&test_retrieval_manifest(
                &identity.project_id,
                symbol_doc_count as i64,
                dense_count as i64,
            ))
            .expect("publish baseline retrieval identity");
        let retrieval = storage
            .get_retrieval_index_publication(&identity.project_id)
            .expect("read baseline retrieval identity")
            .expect("baseline retrieval publication");
        (publication, retrieval)
    };

    for action in [PublicationTestAction::Fail, PublicationTestAction::Cancel] {
        let cancel_token = CancellationToken::new();
        arm_publication_test_fault(PublicationTestBoundary::RuntimeCache, action);
        let outcome = controller
            .republish_semantic_projections_blocking_with_cancel(&cancel_token)
            .expect("post-commit runtime-cache fault must complete publication");
        PUBLICATION_TEST_FAULT.with(|fault| {
            assert!(
                fault.borrow().is_none(),
                "runtime-cache fault was not reached: {action:?}"
            );
        });

        assert_eq!(outcome.previous_publication, previous_publication);
        assert_eq!(
            outcome.publication.generation,
            previous_publication.generation + 1
        );
        assert_eq!(
            outcome.publication.mode,
            IndexPublicationMode::SemanticProjection
        );
        assert_eq!(
            cancel_token.is_cancelled(),
            action == PublicationTestAction::Cancel
        );
        assert_eq!(
            Storage::database_complete_index_publication(&storage_path)
                .expect("read committed semantic publication"),
            Some(outcome.publication.clone())
        );

        let state = controller.state.lock();
        assert!(!state.is_indexing);
        assert!(state.search_engine.is_some());
        assert_eq!(
            state.search_publication.as_ref(),
            Some(&outcome.publication),
            "prepared search state must become the committed runtime generation"
        );
        drop(state);

        let search_generations = persisted_search_generation_names(&storage_path);
        assert!(
            search_generations.contains(&outcome.publication.generation_id),
            "committed search generation must remain current: {search_generations:?}"
        );
        assert!(
            search_generations.len() <= 2,
            "only the current and one complete rollback generation may remain: {search_generations:?}"
        );
        for generation in &search_generations {
            assert!(
                read_search_generation_completion(
                    &search_index_generation_root(&storage_path).join(generation),
                    generation,
                )
                .is_some(),
                "persisted search generation must be complete: {generation}"
            );
        }
        assert_eq!(
            Storage::open(&storage_path)
                .expect("open retained retrieval state")
                .get_retrieval_index_publication(&identity.project_id)
                .expect("read retained retrieval state"),
            Some(retrieval_publication.clone()),
            "semantic projection publication must leave retrieval intentionally stale"
        );
        assert_no_staged_publication_artifacts(&storage_path);
        previous_publication = outcome.publication;
    }
}

#[test]
fn semantic_projection_republish_detects_generation_drift_and_keeps_competing_publication() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let runtime = test_sidecar_runtime_from_env();
    let controller = AppController::new_with_config(runtime.clone());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline core");
    let baseline = Storage::database_complete_index_publication(&storage_path)
        .expect("read baseline")
        .expect("baseline publication");
    let baseline_search = persisted_search_generation_names(&storage_path);
    let baseline_search_generation =
        search_index_path_for_publication(&storage_path, Some(&baseline))
            .expect("baseline search path")
            .file_name()
            .expect("baseline search generation")
            .to_string_lossy()
            .to_string();
    assert!(baseline_search.contains(&baseline_search_generation));
    let competing_publication = Arc::new(std::sync::Mutex::new(None));
    let captured_publication = Arc::clone(&competing_publication);
    let competing_root = workspace.path().to_path_buf();
    let competing_runtime = runtime.clone();
    arm_semantic_projection_before_revalidate_hook(move |path| {
        let competing = AppController::new_with_config(competing_runtime)
            .republish_semantic_projections_at_blocking(competing_root, path.to_path_buf())
            .expect("publish competing generation");
        *captured_publication
            .lock()
            .expect("capture competing publication") = Some(competing.publication);
    });

    let error = match semantic_projection_republish_for_runtime(
        workspace.path(),
        &storage_path,
        None,
        &runtime,
        controller.source_index_policy.as_ref(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("outer writer must detect competing generation"),
    };

    assert_eq!(error.code, "publication_changed");
    let competing = competing_publication
        .lock()
        .expect("read competing publication")
        .clone()
        .expect("competing publication captured");
    assert_eq!(competing.generation, baseline.generation + 1);
    let storage = Storage::open(&storage_path).expect("open competing core");
    assert_eq!(
        storage
            .get_complete_index_publication()
            .expect("read competing core"),
        Some(competing.clone())
    );
    storage
        .validate_dense_anchor_publication(&competing)
        .expect("competing dense publication");
    storage
        .validate_structural_text_unit_publication(&competing)
        .expect("competing structural publication");
    let identity = project_identity_v3(workspace.path());
    storage
        .validate_source_policy_exclusion_publication(
            &competing,
            &identity.project_id,
            &identity.workspace_id,
            default_source_policy_identity(),
        )
        .expect("competing source-policy publication");
    let current_search = persisted_search_generation_names(&storage_path);
    assert!(
        current_search.contains(&baseline_search_generation),
        "the complete rollback search generation must remain usable: baseline={baseline_search:?} current={current_search:?}"
    );
    assert!(
        current_search.contains(&competing.generation_id),
        "the competing complete search generation must remain usable: {current_search:?}"
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn full_refresh_replaces_previous_graph_with_verified_exclusion() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let source_path = workspace.path().join("rust_tictactoe.rs");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let first_storage = Storage::open(&storage_path).expect("first storage");
    let first_publication = first_storage
        .get_complete_index_publication()
        .expect("first publication")
        .expect("complete first publication");
    let first_file = first_storage
        .get_file_by_path(&source_path)
        .expect("first file lookup")
        .expect("indexed Rust source");
    assert!(first_file.complete, "initial source must be verified");
    let first_anchors = first_storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("first anchors")
        .into_iter()
        .filter(|anchor| anchor.file_node_id == Some(CoreNodeId(first_file.id)))
        .map(|anchor| (anchor.node_id, anchor.document_hash, anchor.text))
        .collect::<HashSet<_>>();
    assert!(
        !first_anchors.is_empty(),
        "fixture needs Rust dense anchors"
    );
    drop(first_storage);

    make_source_exceed_default_index_byte_cap(
        &source_path,
        "scheduled but deliberately oversized for this full refresh",
    );
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full refresh should publish the verified exclusion");

    let storage = Storage::open(&storage_path).expect("replacement live storage");
    assert!(
        storage
            .get_file_by_path(&source_path)
            .expect("replacement file lookup")
            .is_none(),
        "excluded content cannot retain a parser-backed file row"
    );
    let publication = storage
        .get_complete_index_publication()
        .expect("replacement publication")
        .expect("complete replacement publication");
    assert_ne!(publication, first_publication);
    let identity = project_identity_v3(workspace.path());
    let manifest = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &identity.project_id,
            &identity.workspace_id,
            default_source_policy_identity(),
        )
        .expect("replacement exclusion manifest");
    assert_eq!(manifest.exclusion_count, 1);
    let retained_anchors = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("replacement anchors")
        .into_iter()
        .filter(|anchor| anchor.file_node_id == Some(CoreNodeId(first_file.id)))
        .map(|anchor| (anchor.node_id, anchor.document_hash, anchor.text))
        .collect::<HashSet<_>>();
    assert!(retained_anchors.is_empty());
    assert!(!first_anchors.is_empty());
}

#[test]
fn full_recovery_publishes_verified_exclusion_and_clears_recovery_fence() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let source_path = workspace.path().join("rust_tictactoe.rs");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let first_storage = Storage::open(&storage_path).expect("first storage");
    let first_publication = first_storage
        .get_complete_index_publication()
        .expect("first publication")
        .expect("complete first publication");
    first_storage
        .begin_incremental_run()
        .expect("mark interrupted incremental run");
    drop(first_storage);

    make_source_exceed_default_index_byte_cap(
        &source_path,
        "recovery candidate is deliberately oversized",
    );
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("verified exclusion should complete full recovery");

    let storage = Storage::open(&storage_path).expect("recovered storage");
    assert!(
        !storage
            .has_incomplete_incremental_run()
            .expect("cleared recovery fence"),
        "complete recovery must clear the interrupted-run fence"
    );
    let publication = storage
        .get_complete_index_publication()
        .expect("recovered complete publication")
        .expect("complete recovered publication");
    assert_ne!(publication, first_publication);
    let identity = project_identity_v3(workspace.path());
    let manifest = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &identity.project_id,
            &identity.workspace_id,
            default_source_policy_identity(),
        )
        .expect("recovered exclusion manifest");
    assert_eq!(manifest.exclusion_count, 1);
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn full_refresh_reuses_unchanged_dense_anchor_inputs_from_previous_live_index() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    let first_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("first full index");
    assert!(
        first_timings.semantic_docs_pending.unwrap_or(0) > 0,
        "initial full refresh should publish pending dense anchor inputs"
    );
    assert_eq!(first_timings.semantic_docs_embedded.unwrap_or(0), 0);
    assert_eq!(first_timings.semantic_docs_reused.unwrap_or(0), 0);

    let first_storage = Storage::open(&storage_path).expect("open first storage");
    let first_docs = first_storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("first dense anchor inputs");
    assert!(
        first_docs.iter().all(|doc| !doc.document_hash.is_empty()
            && doc.policy_version == SEMANTIC_POLICY_VERSION
            && doc.source_identity.starts_with("core:")),
        "dense anchor inputs should carry content, policy, and source reuse identity"
    );
    let first_reuse = first_docs
        .iter()
        .map(|doc| {
            (
                doc.node_id,
                (doc.document_hash.clone(), doc.source_identity.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    assert!(
        first_storage
            .get_all_llm_symbol_docs()
            .expect("legacy docs")
            .is_empty()
    );

    let second_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("second full index");
    assert!(
        second_timings.search_symbol_index_ms.is_some(),
        "staged persisted search timing should be reported"
    );
    assert!(
        second_timings.semantic_doc_build_ms.is_some(),
        "semantic doc build timing should be reported separately"
    );
    assert_eq!(
        second_timings.semantic_docs_embedded.unwrap_or(u32::MAX),
        0,
        "core refreshes must not embed semantic docs"
    );
    assert!(
        second_timings.semantic_docs_reused.unwrap_or(0) > 0,
        "unchanged full refresh should reuse dense anchor content copied into the staged DB"
    );
    assert_eq!(second_timings.semantic_docs_pending.unwrap_or(u32::MAX), 0);
    let second_storage = Storage::open(&storage_path).expect("open second storage");
    let second_docs = second_storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("second dense anchor inputs");
    assert!(second_docs.iter().all(|doc| {
        first_reuse.get(&doc.node_id).is_some_and(|(hash, source)| {
            hash == &doc.document_hash
                && source != &doc.source_identity
                && doc.source_identity.starts_with("core:")
        })
    }));
    assert!(
        second_storage
            .get_all_llm_symbol_docs()
            .expect("legacy docs")
            .is_empty()
    );
}

#[test]
fn unchanged_incremental_refresh_rebuilds_previous_dense_anchor_contract() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let mut contaminated_docs = Storage::open(&storage_path)
        .expect("open storage before contract downgrade")
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs before contract downgrade");
    assert!(
        !contaminated_docs.is_empty(),
        "fixture should persist dense anchor inputs"
    );
    for doc in &mut contaminated_docs {
        doc.policy_version = "graph_first_v0".to_string();
        doc.source_identity = "core:legacy-publication".to_string();
        doc.text
            .push_str("domain_aliases: benchmark-shaped legacy text\n");
        doc.document_hash = format!("legacy-{}", doc.node_id.0);
    }
    let contaminated_count = contaminated_docs.len();
    Storage::open(&storage_path)
        .expect("reopen storage for contract downgrade")
        .upsert_dense_anchor_inputs_batch(&contaminated_docs)
        .expect("persist downgraded dense anchor inputs");

    let mut contaminated_symbol_docs = Storage::open(&storage_path)
        .expect("open graph-native docs before schema downgrade")
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("graph-native docs before schema downgrade");
    assert!(
        !contaminated_symbol_docs.is_empty(),
        "fixture should persist graph-native semantic docs"
    );
    for doc in &mut contaminated_symbol_docs {
        doc.doc_version = LLM_SYMBOL_DOC_SCHEMA_VERSION - 1;
        doc.doc_text
            .push_str("domain_aliases: benchmark-shaped legacy text\n");
    }
    let contaminated_symbol_count = contaminated_symbol_docs.len();
    Storage::open(&storage_path)
        .expect("reopen storage for graph-native schema downgrade")
        .upsert_symbol_search_docs_batch(&contaminated_symbol_docs)
        .expect("persist downgraded graph-native semantic docs");

    let repair_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("unchanged incremental refresh repairs semantic doc schema");
    assert!(
        repair_timings.semantic_docs_pending.unwrap_or(0) >= clamp_usize_to_u32(contaminated_count),
        "contract drift must expand an empty incremental scope and rebuild all dense anchors"
    );
    assert_eq!(repair_timings.semantic_docs_embedded.unwrap_or(0), 0);
    assert!(
        repair_timings.symbol_search_docs_written.unwrap_or(0)
            >= clamp_usize_to_u32(contaminated_symbol_count),
        "schema drift must rebuild all graph-native semantic docs"
    );

    let repaired_docs = Storage::open(&storage_path)
        .expect("open storage after schema repair")
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs after contract repair");
    assert!(
        repaired_docs.iter().all(|doc| {
            doc.policy_version == SEMANTIC_POLICY_VERSION
                && doc.source_identity.starts_with("core:")
                && doc.source_identity != "core:legacy-publication"
                && !doc.document_hash.starts_with("legacy-")
                && !doc.text.contains("domain_aliases:")
        }),
        "unchanged incremental repair should replace every stale dense anchor input"
    );
    let repaired_symbol_docs = Storage::open(&storage_path)
        .expect("open graph-native docs after schema repair")
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("graph-native docs after schema repair");
    assert!(
        repaired_symbol_docs.iter().all(|doc| {
            doc.doc_version == LLM_SYMBOL_DOC_SCHEMA_VERSION
                && !doc.doc_text.contains("domain_aliases:")
        }),
        "unchanged incremental repair should replace every previous-schema graph-native semantic document"
    );
    assert!(
        Storage::open(&storage_path)
            .expect("open legacy vector store")
            .get_all_llm_symbol_docs()
            .expect("legacy semantic docs")
            .is_empty()
    );
}

#[test]
fn unchanged_incremental_refresh_repairs_zero_dense_previous_policy() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let mut storage = Storage::open(&storage_path).expect("open current semantic publication");
    let publication = storage
        .get_complete_index_publication()
        .expect("load core publication")
        .expect("complete core publication");
    assert!(
        storage
            .clear_dense_anchor_inputs()
            .expect("remove current dense anchors")
            > 0,
        "fixture must begin with current dense anchors"
    );
    let legacy_manifest = storage
        .publish_dense_anchor_generation(&publication, "graph_first_v1")
        .expect("publish valid zero-dense previous policy");
    assert_eq!(legacy_manifest.anchor_count, 0);
    assert_eq!(legacy_manifest.policy_version, "graph_first_v1");

    let mut symbol_docs = storage
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("load graph-native docs");
    assert!(!symbol_docs.is_empty(), "fixture must contain symbol docs");
    for doc in &mut symbol_docs {
        doc.policy_version = "graph_first_v1".to_string();
    }
    let symbol_count = symbol_docs.len();
    storage
        .upsert_symbol_search_docs_batch(&symbol_docs)
        .expect("persist previous-policy symbol docs");
    drop(storage);

    let repair_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("unchanged incremental refresh repairs previous policy");
    assert!(
        repair_timings.symbol_search_docs_written.unwrap_or(0) >= clamp_usize_to_u32(symbol_count),
        "symbol-doc policy drift must expand an empty incremental scope"
    );
    assert!(
        repair_timings.semantic_docs_pending.unwrap_or(0) > 0,
        "the repaired policy must be able to publish newly eligible dense anchors"
    );

    let repaired = Storage::open(&storage_path).expect("open repaired semantic publication");
    let repaired_anchors = repaired
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("load repaired dense anchors");
    assert!(!repaired_anchors.is_empty());
    assert!(
        repaired_anchors
            .iter()
            .all(|doc| doc.policy_version == SEMANTIC_POLICY_VERSION)
    );
    assert!(
        repaired
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("load repaired symbol docs")
            .iter()
            .all(|doc| doc.policy_version == SEMANTIC_POLICY_VERSION)
    );
    let repaired_publication = repaired
        .get_complete_index_publication()
        .expect("load repaired core publication")
        .expect("complete repaired publication");
    let repaired_manifest = repaired
        .validate_dense_anchor_publication(&repaired_publication)
        .expect("validate repaired dense publication");
    assert_eq!(repaired_manifest.policy_version, SEMANTIC_POLICY_VERSION);
    assert_eq!(
        repaired_manifest.anchor_count as usize,
        repaired_anchors.len()
    );
}

#[test]
fn full_refresh_repairs_reused_dense_anchors_missing_contract_metadata() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("first full index");

    let mut legacy_docs = Storage::open(&storage_path)
        .expect("open storage before legacy rewrite")
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs before legacy rewrite");
    assert!(
        !legacy_docs.is_empty(),
        "initial full index should persist dense anchor inputs"
    );
    for doc in &mut legacy_docs {
        doc.policy_version.clear();
        doc.source_identity = "core:legacy-unknown".to_string();
    }
    Storage::open(&storage_path)
        .expect("reopen storage for legacy rewrite")
        .upsert_dense_anchor_inputs_batch(&legacy_docs)
        .expect("rewrite legacy dense anchor inputs");

    let repair_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full refresh repairs legacy contract metadata");
    assert!(
        repair_timings.semantic_docs_pending.unwrap_or(0) > 0,
        "missing contract metadata should prevent stale dense anchors from being reused"
    );
    assert_eq!(repair_timings.semantic_docs_embedded.unwrap_or(0), 0);

    let repaired_docs = Storage::open(&storage_path)
        .expect("open storage after repair")
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs after repair");
    assert!(
        repaired_docs.iter().all(|doc| {
            doc.policy_version == SEMANTIC_POLICY_VERSION
                && doc.source_identity.starts_with("core:")
                && doc.source_identity != "core:legacy-unknown"
                && !doc.document_hash.is_empty()
        }),
        "full refresh should backfill dense anchors with the current core contract"
    );
    assert!(
        Storage::open(&storage_path)
            .expect("open legacy vector store")
            .get_all_llm_symbol_docs()
            .expect("legacy semantic docs")
            .is_empty()
    );
}

#[test]
fn incremental_refresh_rebuilds_untouched_dense_anchor_after_cross_file_edge_removal() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace dir");
    let src = workspace.path().join("src");
    fs::create_dir_all(&src).expect("create source directory");
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"semantic-scope-fixture\"\nversion = \"0.1.0\"\n",
    )
    .expect("write package manifest");
    let callee_path = src.join("lib.rs");
    let caller_path = src.join("main.rs");
    fs::write(&callee_path, "pub struct Helper;\n").expect("write callee source");
    fs::write(
        &caller_path,
        "mod lib;\nuse crate::lib::Helper;\npub fn run() -> Helper { Helper }\n",
    )
    .expect("write caller source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full index");

    let first_storage = Storage::open(&storage_path).expect("first storage");
    let first_anchors = first_storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("first dense anchors");
    let first_anchor = first_anchors
        .iter()
        .find(|anchor| {
            anchor.display_name == "Helper" && anchor.file_path.as_deref() == Some("src/lib.rs")
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "callee dense anchor; available={:?}",
                first_anchors
                    .iter()
                    .map(|anchor| (
                        anchor.display_name.as_str(),
                        anchor.file_path.as_deref(),
                        anchor.file_node_id,
                        anchor.selection_reason.as_str(),
                    ))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        first_anchor.text.contains("edge_digest: IMPORT=1"),
        "the initial callee document must expose the cross-file import edge: {}",
        first_anchor.text
    );
    let callee_bytes = fs::read(&callee_path).expect("read untouched callee source");
    drop(first_storage);

    fs::write(&caller_path, "pub fn run() -> i32 { 2 }\n").expect("remove cross-file edge");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental caller refresh");

    assert_eq!(
        fs::read(&callee_path).expect("reread untouched callee source"),
        callee_bytes,
        "the endpoint source must remain byte-for-byte untouched"
    );
    let storage = Storage::open(&storage_path).expect("incremental storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("incremental publication")
        .expect("complete incremental publication");
    storage
        .validate_dense_anchor_publication(&publication)
        .expect("complete dense anchor publication");
    let rebuilt_anchor = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("rebuilt dense anchors")
        .into_iter()
        .find(|anchor| anchor.node_id == first_anchor.node_id)
        .expect("rebuilt callee dense anchor");
    assert_ne!(
        rebuilt_anchor.document_hash, first_anchor.document_hash,
        "removing a cross-file edge must rebuild the connected untouched endpoint"
    );
    assert!(
        !rebuilt_anchor.text.contains("edge_digest: IMPORT=1"),
        "the rebuilt endpoint must not retain the removed cross-file edge: {}",
        rebuilt_anchor.text
    );
}

#[test]
fn incremental_refresh_rebuilds_touched_file_semantic_docs_only() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full index");
    let before_docs = Storage::open(&storage_path)
        .expect("reopen storage before incremental")
        .get_all_llm_symbol_docs()
        .expect("semantic docs before incremental");
    let before_reports = Storage::open(&storage_path)
        .expect("reopen reports before incremental")
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("component reports before incremental")
        .into_iter()
        .filter(|doc| doc.display_name.starts_with("component_report:"))
        .map(|doc| (doc.node_id, doc.doc_hash))
        .collect::<HashMap<_, _>>();

    let rust_fixture = workspace.path().join("rust_tictactoe.rs");
    let mut source = fs::read_to_string(&rust_fixture).expect("read rust fixture");
    source.push_str("\nfn codestory_added_move_hint() -> i32 { 42 }\n");
    fs::write(&rust_fixture, source).expect("write changed rust fixture");

    let incremental_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental index");
    assert!(
        incremental_timings.symbol_search_docs_written.unwrap_or(0) > 0,
        "new symbols from the touched file should update graph-native symbol docs"
    );
    if incremental_timings.semantic_docs_embedded.unwrap_or(0) > 0 {
        assert!(
            incremental_timings
                .semantic_docs_embedded
                .unwrap_or(u32::MAX)
                < clamp_usize_to_u32(before_docs.len()),
            "incremental dense sync should not re-embed untouched files"
        );
    }
    assert_eq!(
        incremental_timings.semantic_docs_stale.unwrap_or(0),
        0,
        "adding a symbol should not make existing semantic docs stale"
    );

    let docs = Storage::open(&storage_path)
        .expect("reopen storage")
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("symbol docs after incremental");
    assert!(
        docs.iter()
            .any(|doc| doc.display_name.contains("codestory_added_move_hint")),
        "incremental symbol docs should include the new symbol"
    );
    assert!(
        docs.iter().any(|doc| {
            doc.display_name.starts_with("component_report:")
                && before_reports
                    .get(&doc.node_id)
                    .is_some_and(|before_hash| before_hash != &doc.doc_hash)
        }),
        "incremental indexing should refresh the affected global component report"
    );
}

#[test]
fn incremental_refresh_removes_stale_component_reports() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace dir");
    for component in ["alpha", "beta"] {
        let component_dir = workspace.path().join(component);
        fs::create_dir_all(&component_dir).expect("create component dir");
        fs::write(
            component_dir.join("lib.rs"),
            format!("pub fn {component}_value() -> i32 {{ 1 }}\n"),
        )
        .expect("write component source");
    }
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full index");

    let full_reports = Storage::open(&storage_path)
        .expect("open full reports")
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("full component reports")
        .into_iter()
        .filter(|doc| doc.display_name.starts_with("component_report:"))
        .map(|doc| (doc.display_name, doc.doc_hash))
        .collect::<HashMap<_, _>>();
    fs::write(
        workspace.path().join("alpha").join("lib.rs"),
        "pub fn alpha_value() -> i32 { 2 }\npub fn alpha_added() {}\n",
    )
    .expect("change alpha source");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental alpha change");
    let changed_reports = Storage::open(&storage_path)
        .expect("open changed reports")
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("changed component reports")
        .into_iter()
        .filter(|doc| doc.display_name.starts_with("component_report:"))
        .map(|doc| (doc.display_name, doc.doc_hash))
        .collect::<HashMap<_, _>>();
    assert_ne!(
        changed_reports.get("component_report:dir:alpha"),
        full_reports.get("component_report:dir:alpha")
    );
    assert_eq!(
        changed_reports.get("component_report:dir:beta"),
        full_reports.get("component_report:dir:beta"),
        "an incremental change should preserve unaffected component reports"
    );

    let before_removal = Storage::open(&storage_path).expect("open changed index");
    let beta_report_id = before_removal
        .get_nodes()
        .expect("component report nodes")
        .into_iter()
        .find(|node| node.serialized_name == "component_report:dir:beta")
        .map(|node| node.id)
        .expect("beta component report");
    let category_id = before_removal
        .create_bookmark_category("Reports")
        .expect("create report bookmark category");
    before_removal
        .add_bookmark(category_id, beta_report_id, Some("temporary report"))
        .expect("bookmark component report");
    drop(before_removal);

    fs::remove_file(workspace.path().join("beta").join("lib.rs")).expect("remove beta source");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental removal");

    let storage = Storage::open(&storage_path).expect("open indexed storage");
    assert!(
        storage
            .get_nodes()
            .expect("nodes after removal")
            .iter()
            .all(|node| node.serialized_name != "component_report:dir:beta")
    );
    assert!(
        storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("symbol docs after removal")
            .iter()
            .all(|doc| doc.display_name != "component_report:dir:beta")
    );
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("dense docs after removal")
            .iter()
            .all(|doc| doc.display_name != "component_report:dir:beta")
    );
    assert!(
        storage
            .get_bookmarks(None)
            .expect("bookmarks after report removal")
            .is_empty(),
        "pruning a stale component report should remove dependent bookmarks"
    );
}

#[test]
fn incremental_refresh_rebuilds_reports_when_path_normalization_changes() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace dir");
    fs::create_dir_all(workspace.path().join("alpha")).expect("create alpha dir");
    fs::write(
        workspace.path().join("alpha").join("lib.rs"),
        "pub fn alpha_value() -> i32 { 1 }\n",
    )
    .expect("write alpha source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full index");

    fs::create_dir_all(workspace.path().join("beta")).expect("create beta dir");
    fs::write(
        workspace.path().join("beta").join("lib.rs"),
        "pub fn beta_value() -> i32 { 2 }\n",
    )
    .expect("write beta source");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental index");

    let report_names = Storage::open(&storage_path)
        .expect("open indexed storage")
        .get_nodes()
        .expect("component report nodes")
        .into_iter()
        .filter(|node| node.serialized_name.starts_with("component_report:"))
        .map(|node| node.serialized_name)
        .collect::<HashSet<_>>();
    assert_eq!(
        report_names,
        HashSet::from([
            "component_report:dir:alpha".to_string(),
            "component_report:dir:beta".to_string(),
        ])
    );
}

#[test]
fn grounding_snapshot_from_summary_open_keeps_search_state_cold() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    {
        let state = controller.state.lock();
        assert!(
            state.search_engine.is_none(),
            "summary open plus indexing should leave search state unloaded"
        );
        assert!(
            state.node_names.is_empty(),
            "summary open plus indexing should leave node label cache empty"
        );
    }

    let snapshot = controller
        .grounding_snapshot(GroundingBudgetDto::Balanced)
        .expect("grounding snapshot");
    let retrieval = snapshot.retrieval.expect("retrieval state");
    assert_eq!(retrieval.mode, RetrievalModeDto::Symbolic);
    assert!(!retrieval.semantic_ready);
    assert_eq!(retrieval.semantic_doc_count, 0);

    let storage = Storage::open(&storage_path).expect("open indexed storage");
    assert!(
        !storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("dense anchor inputs")
            .is_empty(),
        "core indexing should publish embedding-free dense anchor inputs"
    );
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy semantic rows")
            .is_empty(),
        "core indexing should not publish retrieval-owned embeddings"
    );

    let state = controller.state.lock();
    assert!(
        state.search_engine.is_none(),
        "grounding snapshot should not rebuild the full search engine"
    );
    assert!(
        state.node_names.is_empty(),
        "grounding snapshot should not repopulate node labels from search state"
    );
}

#[test]
fn retrieval_state_from_summary_open_keeps_search_state_cold() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let retrieval = controller.retrieval_state().expect("retrieval state");
    assert_eq!(retrieval.mode, RetrievalModeDto::Symbolic);
    assert!(!retrieval.semantic_ready);
    assert_eq!(retrieval.semantic_doc_count, 0);

    let state = controller.state.lock();
    assert!(
        state.search_engine.is_none(),
        "retrieval_state should stay storage-backed on a cold controller"
    );
    assert!(
        state.node_names.is_empty(),
        "retrieval_state should not populate search labels on a cold controller"
    );
}

#[test]
fn search_results_ignores_repo_text_hits_without_full_sidecars() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let source_path = temp.path().join("src").join("lib.rs");
    std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    std::fs::write(
        &source_path,
        "fn alpha() {}\n// this explains how alpha work items flow through the runtime\n",
    )
    .expect("write source");

    {
        let mut storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: source_path.clone(),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 2,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::FILE,
                    serialized_name: source_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "alpha".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project");

    let error = controller
        .search_results(SearchRequest {
            query: "how does alpha work".to_string(),
            repo_text: SearchRepoTextMode::On,
            limit_per_source: 5,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("repo-text search should still require full sidecars");
    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn repo_text_auto_fallback_is_not_product_search_without_full_sidecars() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let source_path = temp.path().join("src").join("lib.rs");
    let readme_path = temp.path().join("README.md");
    std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    std::fs::write(&source_path, "pub fn unrelated_anchor() {}\n").expect("write source");
    std::fs::write(
        &readme_path,
        "GlobalResourceListView is a retired frontend surface mentioned in notes.\n",
    )
    .expect("write readme");

    {
        let mut storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: source_path.clone(),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert source file");
        storage
            .insert_file(&FileInfo {
                id: 12,
                path: readme_path,
                language: "markdown".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert readme file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::FILE,
                    serialized_name: source_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "unrelated_anchor".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project");

    let error = controller
        .search_results(SearchRequest {
            query: "GlobalResourceListView".to_string(),
            repo_text: SearchRepoTextMode::Auto,
            limit_per_source: 5,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("repo-text auto fallback should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn repo_text_ranking_uses_path_and_query_tokens_for_svelte_surfaces() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let rust_path = temp.path().join("src").join("commands.rs");
    let svelte_path = temp.path().join("src").join("App.svelte");
    std::fs::create_dir_all(rust_path.parent().expect("src parent")).expect("create src");
    std::fs::write(
        &rust_path,
        "pub fn get_snapshot() {}\n// invoke runtime bridge\n",
    )
    .expect("write rust");
    std::fs::write(
        &svelte_path,
        "const readSnapshot = () => invoke('get_snapshot');\n",
    )
    .expect("write svelte");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        for (id, path, language) in [(11, rust_path, "rust"), (12, svelte_path.clone(), "svelte")] {
            storage
                .insert_file(&FileInfo {
                    id,
                    path,
                    language: language.to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "readSnapshot get_snapshot App.svelte invoke",
        5,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(
        scan.hits
            .first()
            .is_some_and(|hit| hit.display_name.ends_with("App.svelte")),
        "Svelte command surface should rank first: {:#?}",
        scan.hits
    );
}

#[test]
fn repo_text_partial_matches_surface_public_page_wiring() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let page_path = temp
        .path()
        .join("src")
        .join("app")
        .join("(frontend)")
        .join("posts")
        .join("[slug]")
        .join("page.tsx");
    let social_path = temp.path().join("src").join("lib").join("social-feed.ts");
    std::fs::create_dir_all(page_path.parent().expect("page parent")).expect("create page dir");
    std::fs::create_dir_all(social_path.parent().expect("social parent"))
        .expect("create social dir");
    std::fs::write(
        &page_path,
        "import { PostComments } from './PostComments';\nexport default async function PostPage() { return <PostComments />; }\n",
    )
    .expect("write page");
    std::fs::write(
        &social_path,
        "export async function getElsewhereFeed() { return []; }\n",
    )
    .expect("write social feed");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        for (id, path, language) in [(11, page_path, "tsx"), (12, social_path, "typescript")] {
            storage
                .insert_file(&FileInfo {
                    id,
                    path,
                    language: language.to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "how posts comments auth and elsewhere feed connect to public pages",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(
        scan.hits.iter().any(|hit| hit
            .display_name
            .ends_with("src/app/(frontend)/posts/[slug]/page.tsx")),
        "natural-language repo text should surface public page wiring, not only symbols: {:#?}",
        scan.hits
    );
}

#[test]
fn repo_text_partial_match_requires_distinct_query_terms() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let page_path = temp.path().join("src").join("posts").join("page.tsx");
    std::fs::create_dir_all(page_path.parent().expect("page parent")).expect("create page dir");
    std::fs::write(&page_path, "export const posts = [];\n").expect("write page");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: page_path,
                language: "tsx".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "posts comments auth",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(
        scan.hits.is_empty(),
        "one repeated term in path and file contents should not satisfy multi-concept repo-text matching: {:#?}",
        scan.hits
    );
}

#[test]
fn repo_text_scan_reports_file_cap_on_large_low_match_fixture() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let src = temp.path().join("src");
    std::fs::create_dir_all(&src).expect("create src");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        for idx in 0..(REPO_TEXT_SCAN_FILE_CAP + 3) {
            let path = src.join(format!("file_{idx}.rs"));
            std::fs::write(&path, format!("pub fn file_{idx}() {{}}\n"))
                .expect("write fixture file");
            storage
                .insert_file(&FileInfo {
                    id: idx as i64 + 1,
                    path,
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "needle that is not present",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(scan.hits.is_empty());
    assert!(scan.stats.truncated, "{:?}", scan.stats);
    assert!(scan.stats.scanned_file_count <= REPO_TEXT_SCAN_FILE_CAP as u32);
    assert!(
        scan.stats
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("scanning") || reason.contains("ms"))
    );
    assert!(scan.stats.action.is_some());
}

#[test]
fn repo_text_scan_file_cap_sets_truncated_reason() {
    let mut stats = RepoTextScanStatsDto {
        scanned_file_count: REPO_TEXT_SCAN_FILE_CAP as u32,
        scanned_byte_count: 0,
        skipped_large_file_count: 0,
        file_cap: REPO_TEXT_SCAN_FILE_CAP as u32,
        byte_cap: REPO_TEXT_SCAN_BYTE_CAP as u32,
        time_cap_ms: REPO_TEXT_SCAN_TIME_CAP_MS as u32,
        duration_ms: 0,
        truncated: false,
        reason: None,
        action: None,
    };

    assert!(AppController::repo_text_scan_should_stop(
        &mut stats,
        &Instant::now()
    ));
    assert!(stats.truncated);
    assert!(
        stats
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("scanning 2000 files")),
        "{stats:?}"
    );
    assert!(stats.action.is_some());
}

#[test]
fn repo_text_scan_skips_large_files_before_reading_contents() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let source_path = temp.path().join("large.rs");
    std::fs::write(
        &source_path,
        format!(
            "needle\n{}",
            "x".repeat(REPO_TEXT_MAX_FILE_BYTES as usize + 16)
        ),
    )
    .expect("write large source");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path,
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "needle",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(scan.hits.is_empty());
    assert_eq!(scan.stats.scanned_file_count, 1);
    assert_eq!(scan.stats.scanned_byte_count, 0);
    assert_eq!(scan.stats.skipped_large_file_count, 1);
    assert!(!scan.stats.truncated);
}

#[test]
fn direct_markdown_snippet_is_byte_capped() {
    let text = (0..10_000)
        .map(|idx| format!("line {idx}: {}", "x".repeat(2_048)))
        .collect::<Vec<_>>()
        .join("\n");

    let snippet = bounded_direct_markdown_snippet(&text, Some(5_000), usize::MAX);

    assert!(snippet.truncated);
    assert!(snippet.markdown.len() <= DIRECT_SNIPPET_MAX_BYTES);
    assert!(
        snippet.markdown.contains("snippet truncated by byte cap"),
        "{}",
        snippet.markdown
    );
    assert!(
        snippet.markdown.ends_with("```"),
        "truncated snippet should keep a balanced closing fence:\n{}",
        snippet.markdown
    );
}

#[test]
fn file_backed_snippet_streams_and_caps_long_lines() {
    let temp = tempdir().expect("temp dir");
    let source_path = temp.path().join("long_line.rs");
    std::fs::write(
        &source_path,
        format!("pub fn alpha() {{}}\n// {}\n", "x".repeat(256 * 1024)),
    )
    .expect("write long line source");

    let snippet = bounded_markdown_snippet_from_path(
        &source_path,
        2,
        1,
        DIRECT_SNIPPET_MAX_BYTES,
        DIRECT_SNIPPET_TRUNCATION_SUFFIX,
    )
    .expect("read bounded snippet");

    assert!(snippet.truncated);
    assert!(snippet.markdown.len() <= DIRECT_SNIPPET_MAX_BYTES);
    assert!(snippet.markdown.ends_with("```"));
}

#[test]
fn symbol_context_by_id_does_not_mutate_persisted_semantic_docs() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let before = storage
        .get_llm_symbol_doc_stats()
        .expect("semantic doc stats before");
    let symbol_id = storage
        .get_nodes()
        .expect("load nodes")
        .into_iter()
        .find(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && terminal_symbol_segment(&node_display_name(node)) == "check_winner"
        })
        .map(|node| NodeId::from(node.id))
        .expect("check_winner symbol node");
    drop(storage);

    let context = controller
        .symbol_context(symbol_id.clone())
        .expect("symbol context by id");
    assert_eq!(context.node.id, symbol_id);
    assert!(context.node.display_name.contains("check_winner"));

    let storage = Storage::open(&storage_path).expect("reopen storage after read");
    let after = storage
        .get_llm_symbol_doc_stats()
        .expect("semantic doc stats after");
    assert_eq!(after.doc_count, before.doc_count);
    assert_eq!(after.embedding_model, before.embedding_model);
}

#[test]
fn staged_semantic_finalization_repairs_mixed_dense_anchor_contracts() {
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let mut storage = Storage::new_in_memory().expect("storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);

    let _env = hybrid_test_env();
    let initial_stats = finalize_staged_semantic_docs(&mut storage, None, None, None)
        .expect("initial finalization");
    assert!(initial_stats.docs_pending > 0);
    assert_eq!(initial_stats.docs_embedded, 0);
    let seeded_docs = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("initial dense anchor inputs");
    let mixed_node_id = seeded_docs
        .last()
        .expect("at least one dense anchor input")
        .node_id
        .0;

    storage
        .get_connection()
        .execute(
            "UPDATE dense_anchor_input
             SET policy_version = 'graph_first_v0',
                 source_identity = 'core:legacy-publication',
                 document_hash = 'legacy-document-hash'
             WHERE node_id = ?1",
            [mixed_node_id],
        )
        .expect("mark one dense anchor contract as stale");

    let repair_stats = finalize_staged_semantic_docs(&mut storage, None, None, None)
        .expect("mixed dense anchor contract should force finalization");
    assert!(repair_stats.docs_pending > 0);
    assert_eq!(repair_stats.docs_embedded, 0);

    let docs = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("reloaded dense anchor inputs");
    assert!(!docs.is_empty(), "expected rebuilt dense anchor inputs");
    assert!(
        docs.iter().all(|doc| {
            doc.policy_version == SEMANTIC_POLICY_VERSION
                && doc.source_identity == "core:test-publication"
                && doc.document_hash != "legacy-document-hash"
        }),
        "expected mixed dense anchor inputs to be rebuilt to one core contract"
    );
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy docs")
            .is_empty()
    );
}

#[test]
fn staged_full_semantic_projection_streams_bounded_node_pages() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("generated.rs");
    fs::write(&source_path, "pub fn generated() {}\n").expect("write source");
    let storage_path = temp.path().join("staged.db");
    let mut storage = Storage::open_build(&storage_path).expect("open staged build");
    storage
        .insert_file(&FileInfo {
            id: 1,
            path: source_path.clone(),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        })
        .expect("insert file");
    let mut nodes = vec![
        Node {
            id: CoreNodeId(1),
            kind: NodeKind::FILE,
            serialized_name: source_path.to_string_lossy().to_string(),
            ..Default::default()
        },
        Node {
            id: CoreNodeId(9_000),
            kind: NodeKind::BUILTIN_TYPE,
            serialized_name: "shared_endpoint".to_string(),
            ..Default::default()
        },
    ];
    nodes.extend((0_i64..4_097).map(|offset| Node {
        id: CoreNodeId(10 + offset),
        kind: NodeKind::FUNCTION,
        serialized_name: format!("generated_{offset:04}"),
        qualified_name: Some(format!("generated::generated_{offset:04}")),
        file_node_id: Some(CoreNodeId(1)),
        start_line: Some(1),
        end_line: Some(1),
        ..Default::default()
    }));
    storage
        .insert_nodes_batch(&nodes)
        .expect("insert streamed nodes");
    storage
        .insert_edges_batch(&[
            Edge {
                id: EdgeId(1),
                source: CoreNodeId(10),
                target: CoreNodeId(9_000),
                kind: EdgeKind::CALL,
                file_node_id: Some(CoreNodeId(1)),
                ..Default::default()
            },
            Edge {
                id: EdgeId(2),
                source: CoreNodeId(4_106),
                target: CoreNodeId(9_000),
                kind: EdgeKind::CALL,
                file_node_id: Some(CoreNodeId(1)),
                ..Default::default()
            },
        ])
        .expect("insert shared endpoint edges");

    let _env = hybrid_test_env();
    let stats = finalize_staged_semantic_docs(&mut storage, None, None, None)
        .expect("stream semantic projection");

    assert_eq!(stats.node_load_rows, 4_097);
    assert_eq!(stats.selected_nodes, 4_097);
    assert_eq!(stats.node_stream_batches, 2);
    assert_eq!(stats.endpoint_load_rows, 2);
    assert_eq!(stats.endpoint_load_batches, 2);
    assert_eq!(stats.node_lookup_entries, 4_097);
    assert_eq!(stats.context_file_count, 1);
    assert_eq!(
        storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("load streamed symbol docs")
            .into_iter()
            .filter(|doc| !doc.display_name.starts_with("component_report:"))
            .count(),
        4_097
    );

    let _scope = EnvGuard::set(SEMANTIC_DOC_SCOPE_ENV, "all");
    let all_scope_stats = finalize_staged_semantic_docs(&mut storage, None, None, None)
        .expect("repeat all-symbol stream");
    assert_eq!(all_scope_stats.node_load_rows, 4_098);
    assert_eq!(all_scope_stats.selected_nodes, 4_097);
    assert_eq!(
        storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("load all-scope symbol docs")
            .into_iter()
            .filter(|doc| !doc.display_name.starts_with("component_report:"))
            .count(),
        4_097,
        "retained component-report artifacts must not re-enter the symbol stream"
    );
}

#[test]
fn staged_semantic_graph_context_bounds_high_degree_endpoint_state() {
    const INCIDENT_EDGE_COUNT: i64 = SEMANTIC_EDGE_STREAM_BATCH_SIZE as i64 * 2 + 17;
    const IGNORED_CALL_RAW_TARGET: CoreNodeId = CoreNodeId(90_000);
    const IGNORED_CALL_RESOLVED_TARGET: CoreNodeId = CoreNodeId(90_001);

    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("staged.db");
    let mut storage = Storage::open_build(&storage_path).expect("open staged build");
    let hub = Node {
        id: CoreNodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "hub".to_string(),
        ..Default::default()
    };
    let mut nodes = vec![hub.clone()];
    nodes.extend((0_i64..INCIDENT_EDGE_COUNT).map(|offset| Node {
        id: CoreNodeId(10_000 + offset),
        kind: NodeKind::FUNCTION,
        serialized_name: format!("child_{offset:05}"),
        ..Default::default()
    }));
    nodes.extend([
        Node {
            id: IGNORED_CALL_RAW_TARGET,
            kind: NodeKind::BUILTIN_TYPE,
            serialized_name: "ignored_raw_target".to_string(),
            ..Default::default()
        },
        Node {
            id: IGNORED_CALL_RESOLVED_TARGET,
            kind: NodeKind::FUNCTION,
            serialized_name: "ignored_resolution_must_not_appear".to_string(),
            ..Default::default()
        },
    ]);
    storage
        .insert_nodes_batch(&nodes)
        .expect("insert high-degree nodes");
    let mut edges = vec![Edge {
        id: EdgeId(-1),
        source: hub.id,
        target: IGNORED_CALL_RAW_TARGET,
        kind: EdgeKind::CALL,
        resolved_target: Some(IGNORED_CALL_RESOLVED_TARGET),
        confidence: Some(0.2),
        certainty: Some(codestory_contracts::graph::ResolutionCertainty::Uncertain),
        ..Default::default()
    }];
    edges.extend(
        (0_i64..INCIDENT_EDGE_COUNT)
            .map(|offset| Edge {
                id: EdgeId(offset + 1),
                source: hub.id,
                target: CoreNodeId(10_000 + offset),
                kind: EdgeKind::CALL,
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    );
    storage
        .insert_edges_batch(&edges)
        .expect("insert high-degree edges");
    storage
        .create_semantic_context_endpoint_indexes_for_build()
        .expect("create semantic endpoint indexes");

    let legacy = SemanticDocGraphContext::build_for_scope(
        &storage,
        &[&hub],
        &nodes,
        SemanticDocScope::DurableSymbols,
        HashMap::new(),
        HashMap::new(),
    )
    .expect("build legacy high-degree context");
    let (streamed, stats) = SemanticDocGraphContext::build_for_full_page(
        &storage,
        std::slice::from_ref(&hub),
        SemanticDocScope::DurableSymbols,
        &HashMap::new(),
        &HashMap::new(),
        None,
    )
    .expect("stream high-degree context");

    assert_eq!(streamed.child_labels, legacy.child_labels);
    assert_eq!(streamed.referenced_labels, legacy.referenced_labels);
    assert_eq!(streamed.edge_digests, legacy.edge_digests);
    assert_eq!(streamed.centrality, legacy.centrality);
    assert!(
        streamed
            .child_labels
            .get(&hub.id)
            .is_some_and(Vec::is_empty)
    );
    assert_eq!(
        streamed
            .referenced_labels
            .get(&hub.id)
            .expect("bounded related labels")
            .len(),
        6
    );
    assert_eq!(
        streamed.edge_digests.get(&hub.id),
        Some(&vec![format!("CALL={}", INCIDENT_EDGE_COUNT + 1)])
    );
    assert_eq!(
        streamed.centrality.get(&hub.id),
        Some(&DenseAnchorCentrality {
            child_count: 0,
            related_count: INCIDENT_EDGE_COUNT as usize,
            edge_count: INCIDENT_EDGE_COUNT as usize + 1,
        })
    );
    assert!(dense_anchor_is_central(&streamed, hub.id));
    assert!(
        !streamed
            .referenced_labels
            .get(&hub.id)
            .expect("hub referenced labels")
            .iter()
            .any(|label| label == "ignored_resolution_must_not_appear"),
        "ignored CALL resolution must retain the raw non-indexable endpoint"
    );
    assert!(stats.endpoint_rows >= INCIDENT_EDGE_COUNT as u32);
    assert!(
        stats.lookup_entries <= (SEMANTIC_EDGE_STREAM_BATCH_SIZE + 3) as u32,
        "high-degree endpoint state exceeded one bounded edge batch: {stats:?}"
    );
    assert!(
        stats.endpoint_rows > stats.lookup_entries,
        "telemetry must distinguish cumulative endpoint rows from peak lookup entries"
    );
}

#[test]
fn staged_semantic_graph_context_counts_cross_seed_chunk_edge_once_per_endpoint() {
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("staged.db");
    let mut storage = Storage::open_build(&storage_path).expect("open staged build");
    let nodes = (1_i64..=BUILD_EDGE_SEED_BATCH_SIZE as i64 + 1)
        .map(|id| Node {
            id: CoreNodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("node_{id:03}"),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    storage
        .insert_nodes_batch(&nodes)
        .expect("insert cross-chunk nodes");
    storage
        .insert_edges_batch(&[Edge {
            id: EdgeId(1),
            source: nodes[0].id,
            target: nodes[BUILD_EDGE_SEED_BATCH_SIZE].id,
            kind: EdgeKind::USAGE,
            ..Default::default()
        }])
        .expect("insert cross-chunk edge");
    storage
        .create_semantic_context_endpoint_indexes_for_build()
        .expect("create semantic endpoint indexes");

    let node_refs = nodes.iter().collect::<Vec<_>>();
    let legacy = SemanticDocGraphContext::build_for_scope(
        &storage,
        &node_refs,
        &nodes,
        SemanticDocScope::DurableSymbols,
        HashMap::new(),
        HashMap::new(),
    )
    .expect("build legacy cross-chunk context");
    let (streamed, stats) = SemanticDocGraphContext::build_for_full_page(
        &storage,
        &nodes,
        SemanticDocScope::DurableSymbols,
        &HashMap::new(),
        &HashMap::new(),
        None,
    )
    .expect("stream cross-chunk context");

    assert_eq!(streamed.child_labels, legacy.child_labels);
    assert_eq!(streamed.referenced_labels, legacy.referenced_labels);
    assert_eq!(streamed.edge_digests, legacy.edge_digests);
    for endpoint in [nodes[0].id, nodes[BUILD_EDGE_SEED_BATCH_SIZE].id] {
        assert_eq!(
            streamed.edge_digests.get(&endpoint),
            Some(&vec!["USAGE=1".to_string()])
        );
    }
    assert_eq!(stats.lookup_entries, nodes.len() as u32);
}

#[test]
fn staged_semantic_stream_matches_legacy_bytes_order_pruning_and_component_reports() {
    const SYMBOL_COUNT: i64 = 4_097;
    const FILE_COUNT: i64 = 13;
    const STALE_NODE_ID: CoreNodeId = CoreNodeId(900_000);

    let _env = hybrid_test_env();
    let _tokens = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "8192");
    let temp = tempdir().expect("create temp dir");
    let mut files = Vec::new();
    let mut file_nodes = Vec::new();
    for file_index in 0_i64..FILE_COUNT {
        let file_name = if file_index == 0 {
            "lib.rs".to_string()
        } else {
            format!("unit_{file_index:02}.rs")
        };
        let path = temp.path().join("crates/demo/src").join(file_name);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(&path, format!("pub fn source_{file_index:02}() {{}}\n"))
            .expect("write semantic source fixture");
        let file_id = CoreNodeId(100_000 + file_index);
        files.push(FileInfo {
            id: file_id.0,
            path: path.clone(),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: codestory_store::FileRole::Source,
        });
        file_nodes.push(Node {
            id: file_id,
            kind: NodeKind::FILE,
            serialized_name: path.to_string_lossy().to_string(),
            ..Default::default()
        });
    }
    let symbols = (1_i64..=SYMBOL_COUNT)
        .map(|id| Node {
            id: CoreNodeId(id),
            kind: if id == 1 {
                NodeKind::STRUCT
            } else {
                NodeKind::FUNCTION
            },
            serialized_name: format!("symbol_{id:04}"),
            qualified_name: Some(format!("demo::symbol_{id:04}")),
            file_node_id: Some(CoreNodeId(100_000 + (id - 1) % FILE_COUNT)),
            start_line: Some(1),
            end_line: Some(1),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let external_nodes = (0_i64..10)
        .map(|offset| Node {
            id: CoreNodeId(200_000 + offset),
            kind: NodeKind::BUILTIN_TYPE,
            serialized_name: format!("external_{offset:02}"),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let ranked_node_ids = (1_i64..=9)
        .chain(std::iter::once(SYMBOL_COUNT))
        .collect::<Vec<_>>();
    let edges = ranked_node_ids
        .iter()
        .enumerate()
        .map(|(offset, node_id)| Edge {
            id: EdgeId(offset as i64 + 1),
            source: CoreNodeId(*node_id),
            target: external_nodes[offset].id,
            kind: EdgeKind::CALL,
            resolved_target: (*node_id == SYMBOL_COUNT).then_some(CoreNodeId(2)),
            confidence: (*node_id == SYMBOL_COUNT).then_some(0.2),
            certainty: (*node_id == SYMBOL_COUNT)
                .then_some(codestory_contracts::graph::ResolutionCertainty::Uncertain),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let stale_symbol_doc = SymbolSearchDoc {
        node_id: STALE_NODE_ID,
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "stale".to_string(),
        qualified_name: None,
        file_path: None,
        start_line: None,
        doc_text: "stale".to_string(),
        doc_version: LLM_SYMBOL_DOC_SCHEMA_VERSION,
        doc_hash: "stale".to_string(),
        policy_version: SEMANTIC_POLICY_VERSION.to_string(),
        source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
        updated_at_epoch_ms: 1,
    };
    let stale_dense_input = DenseAnchorInput {
        node_id: STALE_NODE_ID,
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "stale".to_string(),
        qualified_name: None,
        file_path: None,
        start_line: None,
        end_line: None,
        file_role: codestory_store::FileRole::Source,
        source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
        text: "stale".to_string(),
        document_hash: "stale".to_string(),
        selection_reason: DenseAnchorReason::PublicApi.as_str().to_string(),
        policy_version: SEMANTIC_POLICY_VERSION.to_string(),
        source_identity: "core:stale".to_string(),
        updated_at_epoch_ms: 1,
    };

    let seed = |storage: &mut Storage| {
        storage
            .insert_files_batch(&files)
            .expect("insert semantic files");
        let mut nodes = file_nodes.clone();
        nodes.extend(symbols.clone());
        nodes.extend(external_nodes.clone());
        nodes.push(Node {
            id: STALE_NODE_ID,
            kind: NodeKind::FUNCTION,
            serialized_name: "component_report:stale".to_string(),
            canonical_id: Some("codestory:component_report:stale".to_string()),
            ..Default::default()
        });
        storage
            .insert_nodes_batch(&nodes)
            .expect("insert semantic nodes");
        storage
            .insert_component_access_batch(&[(CoreNodeId(1), AccessKind::Public)])
            .expect("insert public semantic access");
        storage
            .insert_edges_batch(&edges)
            .expect("insert semantic edges");
        storage
            .upsert_symbol_search_docs_batch(std::slice::from_ref(&stale_symbol_doc))
            .expect("seed stale symbol doc");
        storage
            .upsert_dense_anchor_inputs_batch(std::slice::from_ref(&stale_dense_input))
            .expect("seed stale dense input");
    };

    let legacy_path = temp.path().join("legacy.db");
    let streamed_path = temp.path().join("streamed.db");
    let mut legacy = Storage::open(&legacy_path).expect("open legacy store");
    let mut streamed = Storage::open_build(&streamed_path).expect("open staged store");
    seed(&mut legacy);
    seed(&mut streamed);
    let legacy_stats = finalize_staged_semantic_docs(&mut legacy, None, None, None)
        .expect("build legacy semantic projection");
    let streamed_stats = finalize_staged_semantic_docs(&mut streamed, None, None, None)
        .expect("build streamed semantic projection");

    let normalize_symbol_docs = |storage: &Storage| {
        let mut docs = storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("load symbol docs");
        for doc in &mut docs {
            doc.updated_at_epoch_ms = 0;
        }
        docs
    };
    let normalize_dense_inputs = |storage: &Storage| {
        let mut inputs = storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("load dense inputs");
        for input in &mut inputs {
            input.updated_at_epoch_ms = 0;
        }
        inputs
    };
    let legacy_docs = normalize_symbol_docs(&legacy);
    let streamed_docs = normalize_symbol_docs(&streamed);
    let legacy_dense = normalize_dense_inputs(&legacy);
    let streamed_dense = normalize_dense_inputs(&streamed);

    assert_eq!(streamed_stats.node_stream_batches, 2);
    assert_eq!(legacy_stats.selected_nodes, SYMBOL_COUNT as u32);
    assert_eq!(streamed_stats.selected_nodes, SYMBOL_COUNT as u32);
    assert_eq!(legacy_stats.docs_stale, 2);
    assert_eq!(streamed_stats.docs_stale, 2);
    assert_eq!(streamed_docs, legacy_docs);
    assert_eq!(streamed_dense, legacy_dense);
    assert!(
        streamed_docs
            .windows(2)
            .all(|pair| pair[0].node_id.0 < pair[1].node_id.0)
    );
    assert!(
        streamed_dense
            .windows(2)
            .all(|pair| pair[0].node_id.0 < pair[1].node_id.0)
    );
    assert!(streamed_docs.iter().all(|doc| doc.node_id != STALE_NODE_ID));
    assert!(
        streamed_dense
            .iter()
            .all(|input| input.node_id != STALE_NODE_ID)
    );
    let dense_reasons = streamed_dense
        .iter()
        .map(|input| input.selection_reason.as_str())
        .collect::<HashSet<_>>();
    assert!(dense_reasons.contains(DenseAnchorReason::PublicApi.as_str()));
    assert!(dense_reasons.contains(DenseAnchorReason::ComponentReport.as_str()));

    let reports = streamed_docs
        .iter()
        .filter(|doc| doc.display_name.starts_with("component_report:"))
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1);
    let report = &reports[0].doc_text;
    assert!(report.contains("symbol_count: 4097"), "{report}");
    assert!(report.contains("file_count: 12"), "{report}");
    assert!(!report.contains("unit_12.rs"), "{report}");
    assert_eq!(
        report
            .lines()
            .filter(|line| line.starts_with("- demo::symbol_"))
            .count(),
        8,
        "{report}"
    );
    assert!(report.contains("- demo::symbol_0008 "), "{report}");
    assert!(!report.contains("- demo::symbol_0009 "), "{report}");
    assert!(!report.contains("- demo::symbol_4097 "), "{report}");
    let dense_report = streamed_dense
        .iter()
        .find(|input| input.node_id == reports[0].node_id)
        .expect("component report dense input");
    assert_eq!(dense_report.text, reports[0].doc_text);
    assert_eq!(dense_report.document_hash, reports[0].doc_hash);
}

#[test]
fn completed_search_cache_load_does_not_mutate_live_semantic_rows() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    storage
        .put_index_publication(&test_index_publication(
            1,
            "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        ))
        .expect("publish identity");
    finalize_staged_semantic_docs(&mut storage, None, None, None).expect("finalize semantic rows");
    let before_legacy = storage
        .get_all_llm_symbol_docs()
        .expect("legacy semantic rows before cache load");
    let before_symbolic = storage
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("symbolic rows before cache load");
    let before_dense = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs before cache load");
    assert!(before_legacy.is_empty());
    assert!(!before_symbolic.is_empty());
    assert!(!before_dense.is_empty());
    storage
        .get_connection()
        .execute_batch(
            "CREATE TRIGGER reject_live_llm_insert BEFORE INSERT ON llm_symbol_doc
             BEGIN SELECT RAISE(ABORT, 'live llm insert'); END;
             CREATE TRIGGER reject_live_llm_update BEFORE UPDATE ON llm_symbol_doc
             BEGIN SELECT RAISE(ABORT, 'live llm update'); END;
             CREATE TRIGGER reject_live_llm_delete BEFORE DELETE ON llm_symbol_doc
             BEGIN SELECT RAISE(ABORT, 'live llm delete'); END;
             CREATE TRIGGER reject_live_symbol_insert BEFORE INSERT ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'live symbol insert'); END;
             CREATE TRIGGER reject_live_symbol_update BEFORE UPDATE ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'live symbol update'); END;
             CREATE TRIGGER reject_live_symbol_delete BEFORE DELETE ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'live symbol delete'); END;
             CREATE TRIGGER reject_live_dense_insert BEFORE INSERT ON dense_anchor_input
             BEGIN SELECT RAISE(ABORT, 'live dense insert'); END;
             CREATE TRIGGER reject_live_dense_update BEFORE UPDATE ON dense_anchor_input
             BEGIN SELECT RAISE(ABORT, 'live dense update'); END;
             CREATE TRIGGER reject_live_dense_delete BEFORE DELETE ON dense_anchor_input
             BEGIN SELECT RAISE(ABORT, 'live dense delete'); END;",
        )
        .expect("install live semantic mutation guards");

    let result = rebuild_search_state_from_storage(&mut storage, &storage_path, None, true)
        .expect("hydrate cache without semantic persistence");

    assert!(!result.engine.semantic_index_ready());
    assert_eq!(result.engine.full_text_doc_count(), result.node_names.len());
    assert_eq!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy semantic rows after cache load"),
        before_legacy
    );
    assert_eq!(
        storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("symbolic rows after cache load"),
        before_symbolic
    );
    assert_eq!(
        storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("dense anchor inputs after cache load"),
        before_dense
    );

    let cancel_token = CancellationToken::new();
    cancel_token.cancel();
    let error = flush_pending_dense_anchor_inputs(
        &mut storage,
        &[pending_semantic_doc_for_test(1, "cancelled")],
        "core:test-publication",
        current_epoch_ms(),
        &mut SemanticProjectionStats::default(),
        Some(&cancel_token),
    )
    .expect_err("cancelled semantic persistence must stop before DB upsert");
    assert_eq!(error.code, "cancelled");
    let error = finalize_staged_semantic_docs(&mut storage, None, None, Some(&cancel_token))
        .expect_err("cancelled semantic finalization must stop before persistence");
    assert_eq!(error.code, "cancelled");
}

#[test]
fn search_generation_path_rejects_invalid_publication_identity() {
    let publication = test_index_publication(1, "../outside");
    let error = search_index_path_for_publication(Path::new("codestory.db"), Some(&publication))
        .expect_err("path-shaped generation identity must be rejected");

    assert_eq!(error.code, "internal");
    assert!(
        error
            .message
            .contains("Invalid index publication generation id")
    );
}

#[test]
fn persisted_search_generations_do_not_overwrite_a_racing_reader() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);

    let old_publication = test_index_publication(1, "11111111-1111-4111-8111-111111111111");
    storage
        .put_index_publication(&old_publication)
        .expect("publish old core generation");
    let old_state = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build old search generation");
    let old_path =
        search_index_path_for_publication(&storage_path, Some(&old_publication)).expect("old path");
    let same_generation_reader = SearchEngine::try_open_existing(&old_path)
        .expect("completed builder must retain only a shared generation lock");
    assert_eq!(same_generation_reader.tantivy_doc_count(), 3);
    drop(same_generation_reader);

    storage
        .insert_nodes_batch(&[Node {
            id: CoreNodeId(4),
            kind: NodeKind::FUNCTION,
            serialized_name: "gamma_generation_anchor".to_string(),
            qualified_name: Some("pkg::gamma_generation_anchor".to_string()),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(8),
            end_line: Some(8),
            ..Default::default()
        }])
        .expect("insert new-generation symbol");
    let new_publication = test_index_publication(2, "22222222-2222-4222-8222-222222222222");
    storage
        .put_index_publication(&new_publication)
        .expect("publish new core generation");
    let new_state = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build new search generation while old reader is live");

    assert!(
        old_state
            .engine
            .search_symbol("gamma_generation_anchor")
            .is_empty(),
        "old reader must remain bound to the old generation"
    );
    assert_eq!(
        new_state.engine.search_symbol("gamma_generation_anchor"),
        vec![CoreNodeId(4)]
    );
    let new_path =
        search_index_path_for_publication(&storage_path, Some(&new_publication)).expect("new path");
    assert!(old_path.is_dir());
    assert!(new_path.is_dir());
    assert_ne!(old_path, new_path);
}

#[test]
fn catalog_waiting_loader_reopens_core_and_search_as_one_generation() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let old_publication = test_index_publication(1, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    storage
        .publish_structural_text_unit_generation(&old_publication)
        .expect("publish old structural text identity");
    storage
        .put_index_publication(&old_publication)
        .expect("publish old identity");
    storage
        .publish_source_policy_exclusion_generation(
            &old_publication,
            "test-project",
            "test-workspace",
            default_source_policy_identity(),
            &[],
        )
        .expect("publish old source policy identity");
    drop(
        rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
            .expect("build old generation"),
    );

    let stale_storage = Storage::open(&storage_path).expect("open pre-publication reader");
    let catalog_guard =
        SearchGenerationCatalogGuard::acquire(&storage_path).expect("hold catalog for publish");
    let loader_path = storage_path.clone();
    let (started_tx, started_rx) = unbounded();
    let loader = std::thread::spawn(move || {
        let mut stale_storage = stale_storage;
        started_tx.send(()).expect("announce loader");
        load_persisted_search_state(&mut stale_storage, &loader_path)
            .expect("load post-publication generation")
    });
    started_rx.recv().expect("loader started");

    let mut staged = SnapshotStore::clone_live_to_staged(&storage_path)
        .expect("clone live database for replacement");
    staged
        .store_mut()
        .get_connection()
        .execute(
            "UPDATE node
             SET serialized_name = 'gamma_generation',
                 qualified_name = 'pkg::gamma_generation'
             WHERE id = 2",
            [],
        )
        .expect("rename symbol in staged core");
    let new_publication = test_index_publication(2, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    staged
        .store_mut()
        .publish_structural_text_unit_generation(&new_publication)
        .expect("publish staged structural text identity");
    staged
        .store_mut()
        .put_index_publication(&new_publication)
        .expect("publish staged identity");
    staged
        .store_mut()
        .publish_source_policy_exclusion_generation(
            &new_publication,
            "test-project",
            "test-workspace",
            default_source_policy_identity(),
            &[],
        )
        .expect("publish staged source policy identity");
    staged
        .publish(&storage_path)
        .expect("publish replacement core");

    let mut live = Storage::open(&storage_path).expect("open replacement core");
    let search_path = search_index_path_for_publication(&storage_path, Some(&new_publication))
        .expect("replacement search path");
    let mut built = build_persisted_search_state_from_canonical_symbols(
        &mut live,
        &search_path,
        false,
        &test_sidecar_runtime_from_env(),
        None,
    )
    .expect("build replacement search generation");
    write_search_generation_completion(
        &search_path,
        &new_publication,
        built.node_names.len(),
        built.engine.tantivy_doc_count(),
    )
    .expect("complete replacement search generation");
    built
        .engine
        .downgrade_persisted_lock_to_shared()
        .expect("share replacement generation");
    drop(catalog_guard);

    let loaded = loader.join().expect("join loader");
    assert_eq!(loaded.publication, Some(new_publication));
    assert_eq!(
        loaded.node_names.get(&CoreNodeId(2)).map(String::as_str),
        Some("pkg::gamma_generation")
    );
    assert!(
        loaded
            .engine
            .search_symbol("gamma_generation")
            .contains(&CoreNodeId(2))
    );
    drop(built);
}

#[test]
fn legacy_search_rebuild_cannot_delete_a_generation_reader() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("codestory.db");
    let legacy_path = search_index_storage_path(&storage_path);
    let mut legacy = SearchEngine::new(Some(&legacy_path)).expect("create legacy search index");
    legacy
        .index_nodes(vec![(CoreNodeId(1), "legacy_symbol".to_string())])
        .expect("index legacy symbol");
    drop(legacy);

    let publication = test_index_publication(1, "88888888-8888-4888-8888-888888888888");
    let generation_path = search_index_path_for_publication(&storage_path, Some(&publication))
        .expect("generation path");
    let mut generation =
        SearchEngine::new(Some(&generation_path)).expect("create generation search index");
    generation
        .index_nodes(vec![(CoreNodeId(2), "generation_symbol".to_string())])
        .expect("index generation symbol");

    let replacement_legacy =
        SearchEngine::new(Some(&legacy_path)).expect("rebuild independent legacy index");

    assert!(generation_path.is_dir());
    assert_eq!(
        generation.search_symbol("generation_symbol"),
        vec![CoreNodeId(2)]
    );
    drop(replacement_legacy);
    drop(generation);
}

#[test]
fn missing_corrupt_or_count_mismatched_search_generation_is_not_rebuilt_by_a_reader() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let publication = test_index_publication(1, "33333333-3333-4333-8333-333333333333");
    storage
        .put_index_publication(&publication)
        .expect("publish core generation");
    let expected_path = search_index_path_for_publication(&storage_path, Some(&publication))
        .expect("expected path");
    let missing_error = match load_persisted_search_state(&mut storage, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must not rebuild a missing search generation"),
    };
    assert_eq!(missing_error.code, "cache_busy");
    assert!(!expected_path.exists());

    storage = Storage::open(&storage_path).expect("reopen writer storage");
    drop(
        rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
            .expect("writer builds expected generation"),
    );
    let completion_path = search_generation_completion_path(&expected_path);
    let correct_completion = fs::read(&completion_path).expect("read completion marker");
    let mut mismatched_completion: SearchGenerationCompletion =
        serde_json::from_slice(&correct_completion).expect("decode completion marker");
    mismatched_completion.symbol_count = mismatched_completion.symbol_count.saturating_add(1);
    fs::write(
        &completion_path,
        serde_json::to_vec(&mismatched_completion).expect("encode mismatched completion"),
    )
    .expect("write mismatched completion marker");

    let mismatch_error = match load_persisted_search_state(&mut storage, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must reject a count-mismatched search generation"),
    };
    assert_eq!(mismatch_error.code, "cache_busy");
    assert!(expected_path.is_dir());

    fs::write(&completion_path, correct_completion).expect("restore completion marker");
    fs::remove_dir_all(&expected_path).expect("remove built generation");
    fs::write(&expected_path, b"corrupt search generation")
        .expect("write corrupt generation artifact");

    let corrupt_error = match load_persisted_search_state(&mut storage, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must not rebuild a corrupt search generation"),
    };

    assert_eq!(corrupt_error.code, "cache_busy");
    assert!(expected_path.is_file());
}

#[test]
fn persisted_loader_reuses_generation_built_by_indexing_finisher() {
    let mut env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let publication = test_index_publication(1, "cccccccc-cccc-4ccc-8ccc-cccccccccccc");
    storage
        .put_index_publication(&publication)
        .expect("publish core identity");

    let finisher_state =
        rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
            .expect("indexing finisher builds search generation");
    assert_eq!(
        finisher_state.search_stats.search_symbol_index_docs_written,
        3
    );
    assert_eq!(
        finisher_state.search_stats.search_symbol_index_writer_count,
        1
    );
    assert_eq!(
        finisher_state.search_stats.search_symbol_index_commit_count,
        1
    );
    assert_eq!(
        finisher_state.search_stats.search_symbol_index_reload_count,
        1
    );
    let reused_state = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("indexing finisher reuses completed search generation");
    assert_eq!(
        reused_state.search_stats.search_symbol_index_docs_written,
        0
    );
    assert_eq!(
        reused_state.search_stats.search_symbol_index_writer_count,
        0
    );
    assert_eq!(
        reused_state.search_stats.search_symbol_index_commit_count,
        0
    );
    assert_eq!(
        reused_state.search_stats.search_symbol_index_reload_count,
        0
    );
    drop(reused_state);
    env.push(EnvGuard::set(
        crate::search::engine::SYMBOL_FULL_TEXT_INDEX_ENV,
        "false",
    ));
    let loaded = load_persisted_search_state(&mut storage, &storage_path)
        .expect("reader loads completed search generation");
    env.pop();

    assert_eq!(loaded.publication, Some(publication));
    assert_eq!(loaded.engine.tantivy_doc_count(), 3);
    assert_eq!(finisher_state.engine.tantivy_doc_count(), 3);
    assert!(
        finisher_state
            .engine
            .search_symbol("alpha")
            .contains(&CoreNodeId(2))
    );
}

#[test]
fn search_generation_retention_keeps_active_and_one_verified_rollback() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("codestory.db");
    let ids = [
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
        "77777777-7777-4777-8777-777777777777",
    ];
    let mut engines = Vec::new();
    for (offset, id) in ids.iter().enumerate() {
        let publication = test_index_publication(offset as u64 + 1, id);
        let path = search_index_path_for_publication(&storage_path, Some(&publication))
            .expect("generation path");
        let mut engine = SearchEngine::new(Some(&path)).expect("create search generation");
        engine
            .index_nodes(vec![(
                CoreNodeId(offset as i64 + 1),
                format!("symbol_{offset}"),
            )])
            .expect("index generation symbol");
        write_search_generation_completion(&path, &publication, 1, engine.tantivy_doc_count())
            .expect("complete search generation");
        engines.push(engine);
    }
    let active_engine = engines.pop().expect("active engine");
    let locked_old_engine = engines.remove(0);
    drop(engines);
    let malformed = search_index_generation_root(&storage_path).join("not-a-generation");
    fs::create_dir_all(&malformed).expect("create malformed generation");
    let malformed_file = search_index_generation_root(&storage_path).join("partial-generation");
    fs::write(&malformed_file, b"partial").expect("create partial generation artifact");
    let partial_publication = test_index_publication(8, "99999999-9999-4999-8999-999999999999");
    let partial_path = search_index_path_for_publication(&storage_path, Some(&partial_publication))
        .expect("partial generation path");
    let mut partial =
        SearchEngine::new(Some(&partial_path)).expect("create crash-partial generation");
    partial
        .index_nodes(vec![(CoreNodeId(99), "partial_symbol".to_string())])
        .expect("commit partial generation batch");
    drop(partial);
    let partial_lock_path = crate::search::engine::persisted_search_index_lock_path(&partial_path);

    prune_search_generations(&storage_path, ids[3]).expect("prune with locked reader");
    assert!(!malformed.exists());
    assert!(!malformed_file.exists());
    assert!(
        !partial_path.exists(),
        "structurally openable generation without completion marker is not a rollback"
    );
    assert!(
        partial_lock_path.is_file(),
        "generation lock files must remain durable after data pruning"
    );
    let first_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&partial_lock_path)
        .expect("open first durable lock handle");
    let second_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&partial_lock_path)
        .expect("open second durable lock handle");
    assert!(FileExt::try_lock_exclusive(&first_lock).expect("lock first handle"));
    assert!(
        !FileExt::try_lock_exclusive(&second_lock).expect("contend second handle"),
        "both handles must coordinate through the same durable lock file"
    );
    FileExt::unlock(&first_lock).expect("unlock first handle");
    assert!(FileExt::try_lock_exclusive(&second_lock).expect("lock second handle after release"));
    FileExt::unlock(&second_lock).expect("unlock second handle");
    assert!(
        search_index_generation_root(&storage_path)
            .join(ids[0])
            .is_dir(),
        "locked old generation must be skipped"
    );

    drop(locked_old_engine);
    prune_search_generations(&storage_path, ids[3]).expect("prune unlocked generations");
    let retained = fs::read_dir(search_index_generation_root(&storage_path))
        .expect("list retained generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 2, "active plus one rollback should remain");
    assert!(
        retained.iter().any(|entry| entry.file_name() == ids[3]),
        "active generation must remain"
    );
    drop(active_engine);
}

#[test]
fn search_without_publication_identity_uses_legacy_storage_path() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);

    let rebuilt = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build legacy search index");

    assert!(search_index_storage_path(&storage_path).is_dir());
    assert!(!search_index_generation_root(&storage_path).exists());
    assert!(
        rebuilt
            .engine
            .search_symbol("beta")
            .contains(&CoreNodeId(3))
    );
}

#[test]
fn merge_search_hits_by_node_id_keeps_stronger_expanded_score() {
    let mut hits = vec![
        SearchHit {
            node_id: NodeId("primary".to_string()),
            display_name: "alpha".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(10),
            score: 0.25,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("secondary".to_string()),
            display_name: "alpha".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(20),
            score: 0.75,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
    ];

    merge_search_hits_by_node_id(
        &mut hits,
        vec![SearchHit {
            node_id: NodeId("primary".to_string()),
            display_name: "alpha".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(10),
            score: 250.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }],
    );

    hits.sort_by(|left, right| compare_search_hits("alpha", left, right));

    assert_eq!(hits[0].node_id, NodeId("primary".to_string()));
    assert_eq!(hits[0].score, 250.0);
}

#[test]
fn embedded_exact_symbol_terms_count_and_annotate_exact_hits() {
    let mut hit = SearchHit {
        node_id: NodeId("search-hybrid".to_string()),
        display_name: "SearchEngine::search_hybrid_with_scores".to_string(),
        kind: codestory_contracts::api::NodeKind::METHOD,
        file_path: Some("src/search/engine.rs".to_string()),
        line: Some(1769),
        score: 0.25,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    };
    let query = "exact symbol first semantic ranking search_hybrid_with_scores";

    assert_eq!(exact_symbol_hit_count(query, std::slice::from_ref(&hit)), 1);

    annotate_search_hit_match_quality(query, std::slice::from_mut(&mut hit));

    assert_eq!(
        hit.match_quality,
        Some(codestory_contracts::api::SearchMatchQualityDto::NormalizedExact)
    );
}

#[test]
fn primary_source_retention_keeps_short_precise_windows() {
    assert_eq!(primary_source_retention_threshold(1), 1);
    assert_eq!(primary_source_retention_threshold(3), 3);
    assert_eq!(primary_source_retention_threshold(10), 3);
    assert_eq!(primary_source_retention_threshold(50), 3);
}

#[test]
fn inexact_search_results_deduplicate_repeated_display_keys() {
    let mut hits = vec![
        SearchHit {
            node_id: NodeId("embedding-engine-id".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(178),
            score: 0.90,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("embedding-engine-id-copy".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(187),
            score: 0.80,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("other-helper".to_string()),
            display_name: "EmbeddingEngineCache::open".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(194),
            score: 0.70,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
    ];

    hits.sort_by(|left, right| {
        compare_search_hits(
            "embedding engine identity parser configuration",
            left,
            right,
        )
    });
    dedupe_inexact_search_hits_by_display_key(
        "embedding engine identity parser configuration",
        &mut hits,
    );

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].node_id, NodeId("embedding-engine-id".to_string()));
    assert_eq!(hits[1].node_id, NodeId("other-helper".to_string()));
}

#[test]
fn exact_search_results_keep_repeated_display_keys() {
    let mut hits = vec![
        SearchHit {
            node_id: NodeId("embedding-engine-id".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(178),
            score: 0.90,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("embedding-engine-id-copy".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(187),
            score: 0.80,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
    ];

    dedupe_inexact_search_hits_by_display_key("EMBEDDING_ENGINE_ID", &mut hits);

    assert_eq!(hits.len(), 2);
}

#[test]
fn exact_symbol_queries_skip_primary_source_pretruncate() {
    assert!(
        !should_pretruncate_primary_source_window("StorageAccess", true, 250, 10),
        "exact symbol queries need final exact-symbol sorting before truncation"
    );
    assert!(should_pretruncate_primary_source_window(
        "how search ranking works",
        true,
        250,
        10
    ));
    assert!(!should_pretruncate_primary_source_window(
        "how search ranking works",
        false,
        250,
        10
    ));
}

#[test]
fn exact_symbol_fast_path_is_conservative() {
    let req = |query: &str,
               hybrid_weights: Option<AgentHybridWeightsDto>,
               hybrid_limits: Option<SearchHybridLimitsDto>| SearchRequest {
        query: query.to_string(),
        repo_text: SearchRepoTextMode::Off,
        limit_per_source: 10,
        expand_search_plan: false,
        hybrid_weights,
        hybrid_limits,
    };

    assert!(exact_symbol_lexical_fast_path(
        &req("Workbench", None, None),
        None
    ));
    assert!(exact_symbol_lexical_fast_path(
        &req("Subcommand::Exec", None, None),
        None
    ));
    assert!(exact_symbol_lexical_fast_path(
        &req("check_winner", None, None),
        None
    ));
    assert!(!exact_symbol_lexical_fast_path(
        &req("authorization", None, None),
        None
    ));
    assert!(!exact_symbol_lexical_fast_path(
        &req("how ExtensionService starts", None, None),
        None
    ));
    assert!(!exact_symbol_lexical_fast_path(
        &req(
            "Workbench",
            None,
            Some(SearchHybridLimitsDto {
                lexical: None,
                semantic: Some(20),
            }),
        ),
        None
    ));

    let weights = AgentHybridWeightsDto {
        lexical: Some(0.25),
        semantic: Some(0.75),
        graph: None,
    };
    assert!(!exact_symbol_lexical_fast_path(
        &req("Workbench", Some(weights.clone()), None),
        Some(&weights)
    ));
}

#[test]
fn exact_symbol_merged_lexical_queries_dedupe_exact_anchor_scan() {
    assert_eq!(
        exact_symbol_merged_lexical_queries("Workbench"),
        vec!["Workbench".to_string()]
    );
    assert_eq!(
        exact_symbol_merged_lexical_queries("Subcommand::Exec"),
        vec!["Subcommand::Exec".to_string(), "Exec".to_string()]
    );
    assert_eq!(
        exact_symbol_merged_lexical_queries("how ExtensionHostManager starts"),
        vec!["how ExtensionHostManager starts".to_string()]
    );
}

#[test]
fn mixed_natural_language_query_detects_embedded_symbol_prompts() {
    assert!(mixed_natural_language_query(
        "how ExtensionHostManager starts"
    ));
    assert!(!mixed_natural_language_query("Workbench"));
    assert!(!mixed_natural_language_query("Subcommand::Exec"));
}

#[test]
fn hybrid_search_config_skips_exact_symbol_escalation_for_mixed_nl() {
    let req = SearchRequest {
        query: "how ExtensionHostManager starts".to_string(),
        repo_text: SearchRepoTextMode::Off,
        limit_per_source: 10,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    };
    let config = hybrid_search_config_for_request(&req, 10, None, true);
    assert_eq!(config.max_results, 10);
}

#[test]
fn exact_symbol_fast_path_returns_lexical_hits_without_semantic_fallback() {
    let mut engine = SearchEngine::new(None).expect("search engine");
    engine
        .index_nodes(vec![(CoreNodeId(1), "Workbench".to_string())])
        .expect("index nodes");
    let req = SearchRequest {
        query: "Workbench".to_string(),
        repo_text: SearchRepoTextMode::Off,
        limit_per_source: 10,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    };
    let storage_retrieval = RetrievalStateDto {
        mode: RetrievalModeDto::Hybrid,
        hybrid_configured: true,
        semantic_ready: true,
        semantic_mode: SemanticModeDto::Enabled,
        semantic_doc_count: 170_000,
        embedding_model: Some("test-model".to_string()),
        current_embedding: None,
        stored_embedding: None,
        fallback_reason: None,
        fallback_message: None,
    };
    let graph_boosts = HashMap::new();
    let mut retrieval = storage_retrieval.clone();
    let use_exact_symbol_lexical_fast_path = exact_symbol_lexical_fast_path(&req, None);

    let hits = hybrid_hits_for_retrieval_state(
        &mut engine,
        HybridHitsContext {
            req: &req,
            graph_boosts: &graph_boosts,
            requested_max_results: 10,
            request_weights: None,
            prefer_primary_sources: true,
            storage_retrieval: &storage_retrieval,
            use_exact_symbol_lexical_fast_path,
        },
        &mut retrieval,
    );

    assert!(use_exact_symbol_lexical_fast_path);
    assert_eq!(hits.first().map(|hit| hit.node_id), Some(CoreNodeId(1)));
    assert_eq!(hits[0].semantic_score, 0.0);
    assert_eq!(retrieval.fallback_reason, None);
    assert_eq!(retrieval.fallback_message, None);
}

#[test]
fn zero_semantic_request_weights_use_lexical_hits_without_semantic_fallback() {
    let mut engine = SearchEngine::new(None).expect("search engine");
    engine
        .index_nodes(vec![(CoreNodeId(1), "ExtensionHostManager".to_string())])
        .expect("index nodes");
    let req = SearchRequest {
        query: "ExtensionHostManager".to_string(),
        repo_text: SearchRepoTextMode::Off,
        limit_per_source: 10,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    };
    let storage_retrieval = RetrievalStateDto {
        mode: RetrievalModeDto::Hybrid,
        hybrid_configured: true,
        semantic_ready: true,
        semantic_mode: SemanticModeDto::Enabled,
        semantic_doc_count: 170_000,
        embedding_model: Some("test-model".to_string()),
        current_embedding: None,
        stored_embedding: None,
        fallback_reason: None,
        fallback_message: None,
    };
    let graph_boosts = HashMap::new();
    let mut retrieval = storage_retrieval.clone();
    let request_weights = AgentHybridWeightsDto {
        lexical: Some(1.0),
        semantic: Some(0.0),
        graph: Some(0.0),
    };

    let hits = hybrid_hits_for_retrieval_state(
        &mut engine,
        HybridHitsContext {
            req: &req,
            graph_boosts: &graph_boosts,
            requested_max_results: 10,
            request_weights: Some(request_weights),
            prefer_primary_sources: true,
            storage_retrieval: &storage_retrieval,
            use_exact_symbol_lexical_fast_path: false,
        },
        &mut retrieval,
    );

    assert_eq!(hits.first().map(|hit| hit.node_id), Some(CoreNodeId(1)));
    assert_eq!(hits[0].semantic_score, 0.0);
    assert_eq!(retrieval.fallback_reason, None);
    assert_eq!(retrieval.fallback_message, None);
}

#[test]
fn exact_symbol_merged_lexical_hits_include_terminal_symbol_matches() {
    let mut engine = SearchEngine::new(None).expect("search engine");
    engine
        .index_nodes(vec![
            (CoreNodeId(1), "exec_events::ThreadEvent".to_string()),
            (CoreNodeId(2), "ThreadEvent".to_string()),
            (
                CoreNodeId(3),
                "crate::exec_events::ThreadEvent (import)".to_string(),
            ),
        ])
        .expect("index nodes");

    let hits = exact_symbol_merged_lexical_hybrid_hits(
        &engine,
        "exec_events::ThreadEvent",
        &HashMap::new(),
    );
    let ids = hits.iter().map(|hit| hit.node_id).collect::<Vec<_>>();

    assert!(
        ids.contains(&CoreNodeId(2)),
        "terminal exact symbol should be admitted beside qualified aliases: {ids:?}"
    );
    assert_eq!(
        ids.iter().filter(|id| **id == CoreNodeId(2)).count(),
        1,
        "exact-symbol merging should preserve node uniqueness: {ids:?}"
    );
}

#[test]
fn full_index_rebuilds_semantic_docs_when_source_text_changes() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace dir");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    write_reindex_semantic_fixture(workspace.path(), "initial compressed digest");
    controller
        .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path.clone())
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let storage = Storage::open(&storage_path).expect("open storage after initial index");
    let initial_docs = storage
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("load initial symbol docs")
        .into_iter()
        .filter(|doc| doc.display_name == "build_snapshot_digest")
        .collect::<Vec<_>>();
    assert!(!initial_docs.is_empty(), "initial digest doc");
    assert!(
        initial_docs
            .iter()
            .any(|doc| doc.doc_text.contains("initial_compressed_digest")),
        "initial digest docs should include fixture source text: {:?}",
        initial_docs
            .iter()
            .map(|doc| doc.doc_text.as_str())
            .collect::<Vec<_>>()
    );
    drop(storage);

    write_reindex_semantic_fixture(workspace.path(), "updated compressed digest");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("rerun full index");

    let storage = Storage::open(&storage_path).expect("open storage after rerun");
    let updated_docs = storage
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("load updated symbol docs")
        .into_iter()
        .filter(|doc| doc.display_name == "build_snapshot_digest")
        .collect::<Vec<_>>();
    assert!(!updated_docs.is_empty(), "updated digest doc");
    assert!(
        updated_docs
            .iter()
            .any(|doc| doc.doc_text.contains("updated_compressed_digest")),
        "updated digest docs should include fixture source text: {:?}",
        updated_docs
            .iter()
            .map(|doc| doc.doc_text.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        !updated_docs
            .iter()
            .any(|doc| doc.doc_text.contains("initial_compressed_digest")),
        "full index should rebuild symbol docs instead of reusing stale persisted content"
    );
}

#[test]
fn finalize_indexing_without_runtime_refresh_propagates_rebuild_failure() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");

    {
        let mut state = controller.state.lock();
        state.is_indexing = true;
        state
            .node_names
            .insert(CoreNodeId(999), "stale_symbol".to_string());
        let engine = SearchEngine::new(None).expect("search engine");
        publish_search_engine(&mut state, engine, None);
    }

    let error = controller
        .finalize_indexing_without_runtime_refresh_with(&storage_path, None, |_storage, _| {
            Err(ApiError::internal("forced rebuild failure".to_string()))
        })
        .expect_err("forced rebuild failure should propagate");

    assert_eq!(error.code, "internal");
    assert_eq!(error.message, "forced rebuild failure");

    let state = controller.state.lock();
    assert!(!state.is_indexing);
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

fn empty_indexing_run_summary() -> IndexingRunSummary {
    IndexingRunSummary {
        phase_timings: IndexingPhaseTimings::default(),
        staged_semantic_stats: SemanticProjectionStats::default(),
        llm_refresh_scope: None,
        publication: IndexPublicationRecord {
            generation: 1,
            generation_id: "11111111-1111-4111-8111-111111111111".to_string(),
            run_id: "test-run".to_string(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        },
        prepared_search_state: None,
    }
}

fn persisted_empty_indexing_run_summary(storage_path: &Path) -> IndexingRunSummary {
    let summary = empty_indexing_run_summary();
    Storage::open(storage_path)
        .expect("open storage for publication identity")
        .put_index_publication(&summary.publication)
        .expect("persist test publication identity");
    summary
}

#[test]
fn successful_index_refresh_clears_indexing_state() {
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("codestory.db");
    drop(Storage::open(&storage_path).expect("seed storage"));
    let controller = AppController::new();

    {
        let mut state = controller.state.lock();
        state.is_indexing = true;
    }

    let summary = persisted_empty_indexing_run_summary(&storage_path);
    let timings = controller
        .finish_successful_indexing(summary, &storage_path, true, None)
        .expect("cache refresh should succeed");

    assert!(timings.cache_refresh_ms.is_some());
    assert!(!controller.state.lock().is_indexing);
}

#[test]
fn async_incremental_finishes_cache_boundary_before_clearing_marker() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish compatible baseline");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn value() -> i32 { 2 }\n",
    )
    .expect("modify source");
    let events = controller.events();

    controller
        .start_indexing(StartIndexingRequest {
            mode: IndexMode::Incremental,
        })
        .expect("start async incremental");

    let phase_timings = loop {
        match events
            .recv_timeout(Duration::from_secs(30))
            .expect("async indexing terminal event")
        {
            AppEventPayload::IndexingComplete { phase_timings, .. } => break phase_timings,
            AppEventPayload::IndexingFailed { error } => {
                panic!("async incremental failed: {error}")
            }
            _ => {}
        }
    };

    assert!(phase_timings.publish_ms.is_some());
    assert!(phase_timings.cache_refresh_ms.is_some());
    let storage = Storage::open(&storage_path).expect("open published storage");
    assert!(
        !storage
            .has_incomplete_incremental_run()
            .expect("marker after async completion")
    );
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("schema after async completion"),
        codestory_store::CURRENT_SCHEMA_VERSION
    );
    assert!(!controller.state.lock().is_indexing);
}

#[test]
fn empty_full_refresh_reports_adaptive_chunk_config() {
    let workspace = tempdir().expect("workspace dir");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project");

    let timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("empty full publication");

    assert_eq!(
        timings.full_refresh_chunk_target_bytes,
        Some(8 * 1024 * 1024)
    );
    assert_eq!(timings.full_refresh_chunk_target_nodes, Some(120_000));
    assert_eq!(timings.full_refresh_chunk_file_ceiling, Some(512));
    assert_eq!(timings.full_refresh_chunk_max_files, Some(0));
    assert_eq!(timings.full_refresh_chunk_max_planned_bytes, Some(0));
    assert_eq!(timings.full_refresh_chunk_max_nodes, Some(0));
    assert_eq!(timings.full_refresh_chunk_budget_overruns, Some(0));
    assert!(timings.full_refresh_chunk_planning_ms.is_some());
}

#[test]
fn full_and_incremental_publications_advance_one_durable_generation() {
    let assert_promotion_reconciles = |promotion: &CorePromotionTimings| {
        let named_ms = promotion
            .lock_recovery_ms
            .saturating_add(promotion.candidate_validation_ms)
            .saturating_add(promotion.previous_validation_ms)
            .saturating_add(promotion.rollback_backup_copy_ms.unwrap_or_default())
            .saturating_add(promotion.backup_validation_ms.unwrap_or_default())
            .saturating_add(promotion.prepared_journal_write_ms)
            .saturating_add(promotion.prepared_journal_file_sync_ms)
            .saturating_add(promotion.prepared_journal_directory_sync_ms)
            .saturating_add(promotion.staged_to_live_restore_ms)
            .saturating_add(promotion.promoted_validation_ms)
            .saturating_add(promotion.committed_journal_ms)
            .saturating_add(promotion.cleanup_ms);
        assert_eq!(
            named_ms.saturating_add(promotion.unattributed_ms),
            promotion.total_ms
        );
    };
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn first_value() -> i32 { 1 }\n",
    )
    .expect("write initial source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");

    let full_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("first full publication");
    let parser_cache = full_timings
        .parser_artifact_cache
        .as_ref()
        .expect("parser cache telemetry");
    assert_eq!(parser_cache.policy, ArtifactCachePolicyDto::KnownEmpty);
    assert_eq!(parser_cache.logical_lookups, 1);
    assert_eq!(parser_cache.physical_queries, 0);
    assert_eq!(parser_cache.hits, 0);
    assert_eq!(parser_cache.misses, 1);
    assert_eq!(parser_cache.reader_opens, 0);
    assert_eq!(parser_cache.lookup_wall_ms, 0);
    let structural_cache = full_timings
        .structural_artifact_cache
        .as_ref()
        .expect("structural cache telemetry");
    assert_eq!(structural_cache.policy, ArtifactCachePolicyDto::KnownEmpty);
    assert_eq!(structural_cache.logical_lookups, 0);
    assert_eq!(structural_cache.physical_queries, 0);
    assert_eq!(structural_cache.reader_opens, 0);
    assert_eq!(full_timings.full_refresh_queue_capacity, Some(1));
    assert_eq!(full_timings.full_refresh_queue_high_water, Some(1));
    assert_eq!(full_timings.full_refresh_chunks_produced, Some(1));
    assert_eq!(full_timings.full_refresh_chunks_persisted, Some(1));
    assert!(full_timings.full_refresh_producer_blocked_ms.is_some());
    assert!(full_timings.full_refresh_writer_idle_ms.is_some());
    assert_eq!(
        full_timings.full_refresh_chunk_target_bytes,
        Some(8 * 1024 * 1024)
    );
    assert_eq!(full_timings.full_refresh_chunk_target_nodes, Some(120_000));
    assert_eq!(full_timings.full_refresh_chunk_file_ceiling, Some(512));
    assert_eq!(full_timings.full_refresh_chunk_max_files, Some(1));
    assert!(full_timings.full_refresh_chunk_max_planned_bytes.is_some());
    assert!(full_timings.full_refresh_chunk_max_nodes.is_some());
    assert_eq!(full_timings.full_refresh_chunk_budget_overruns, Some(0));
    assert!(full_timings.full_refresh_chunk_planning_ms.is_some());
    let full_projection = full_timings
        .projection_persistence
        .as_ref()
        .expect("full projection persistence telemetry");
    assert_eq!(
        Some(full_projection.transactions),
        full_timings.projection_batch_transactions
    );
    assert!(full_projection.row_attempts > 0);
    assert!(full_projection.bound_bytes > 0);
    assert!(full_projection.statement_executions > 0);
    assert_eq!(
        full_projection.dirty_state.statement_executions,
        u64::from(full_projection.transactions) * 4
    );
    assert_eq!(
        full_timings.staged_sqlite_wal_autocheckpoint_bytes,
        Some(64 * 1024 * 1024)
    );
    assert!(full_timings.staged_sqlite_checkpoint_ms.is_some());
    assert!(full_timings.staged_sqlite_sync_ms.is_some());
    assert!(full_timings.staged_snapshot_copy.is_none());
    let full_promotion = full_timings
        .core_promotion
        .as_ref()
        .expect("first full promotion telemetry");
    assert!(full_promotion.previous_live_bytes.is_none());
    assert!(full_promotion.rollback_backup_copy_ms.is_none());
    assert!(full_promotion.backup_validation_ms.is_none());
    assert!(full_promotion.rollback_backup_bytes.is_none());
    assert!(full_promotion.candidate_bytes > 0);
    assert_promotion_reconciles(full_promotion);
    assert_eq!(full_timings.search_projection_rebuild_ms, Some(0));
    assert!(full_timings.search_symbol_stream_ms.is_some());
    assert!(
        full_timings
            .search_symbol_stream_rows
            .is_some_and(|rows| rows > 0)
    );
    assert_eq!(full_timings.search_symbol_stream_batches, Some(1));
    assert_eq!(full_timings.search_symbol_index_writer_count, Some(1));
    assert_eq!(full_timings.search_symbol_index_commit_count, Some(1));
    assert_eq!(full_timings.search_symbol_index_reload_count, Some(1));
    assert!(full_timings.search_symbol_index_commit_ms.is_some());
    assert!(full_timings.search_symbol_index_reload_ms.is_some());
    assert!(full_timings.semantic_context_index_ms.is_some());
    assert!(full_timings.semantic_node_load_ms.is_some());
    assert!(
        full_timings.deferred_indexes_ms.unwrap_or_default()
            >= full_timings.semantic_context_index_ms.unwrap_or_default()
    );
    assert!(
        full_timings
            .semantic_node_load_rows
            .is_some_and(|rows| rows > 0)
    );
    assert!(
        full_timings
            .semantic_node_stream_batches
            .is_some_and(|batches| batches > 0)
    );
    assert!(full_timings.semantic_endpoint_load_ms.is_some());
    assert!(full_timings.semantic_endpoint_load_rows.is_some());
    assert!(full_timings.semantic_endpoint_load_batches.is_some());
    assert!(
        full_timings
            .semantic_selected_nodes
            .is_some_and(|rows| rows > 0)
    );
    assert_eq!(
        full_timings.semantic_selected_nodes,
        full_timings.semantic_node_load_rows
    );
    assert!(
        full_timings
            .semantic_context_file_count
            .is_some_and(|files| files > 0)
    );
    assert!(
        full_timings
            .semantic_context_path_bytes
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(
        full_timings
            .semantic_node_lookup_entries
            .is_some_and(|peak| {
                peak <= full_timings
                    .semantic_node_load_rows
                    .unwrap_or_default()
                    .saturating_add(full_timings.semantic_endpoint_load_rows.unwrap_or_default())
            })
    );
    assert!(full_timings.semantic_context_ms.is_some());
    let full_refresh_wall = full_timings
        .full_refresh_wall
        .as_ref()
        .expect("full refresh wall accounting");
    let accounted_ms = full_refresh_wall
        .live_inspection_ms
        .saturating_add(full_refresh_wall.source_discovery_ms)
        .saturating_add(full_refresh_wall.stage_open_ms)
        .saturating_add(full_refresh_wall.indexer_execution_ms)
        .saturating_add(full_refresh_wall.coverage_validation_ms)
        .saturating_add(full_refresh_wall.copy_forward_ms)
        .saturating_add(full_refresh_wall.semantic_stage_ms)
        .saturating_add(full_refresh_wall.snapshot_stage_ms)
        .saturating_add(full_refresh_wall.publication_prepare_ms)
        .saturating_add(full_refresh_wall.search_generation_ms)
        .saturating_add(full_refresh_wall.catalog_publication_ms)
        .saturating_add(full_refresh_wall.unattributed_ms);
    assert!(accounted_ms <= full_refresh_wall.core_refresh_ms);
    assert_eq!(
        Storage::open(&storage_path)
            .expect("open full publication")
            .get_search_symbol_projection_count()
            .expect("count legacy search projection"),
        0,
        "fresh full publication must not materialize the legacy projection table"
    );
    let first = controller
        .index_publication()
        .expect("read first publication")
        .expect("first publication identity");
    assert_eq!(first.generation, 1);
    assert_eq!(first.mode, IndexPublicationMode::Full);

    fs::write(
        workspace.path().join("second.rs"),
        "pub fn second_value() -> i32 { 2 }\n",
    )
    .expect("write incremental source");
    let incremental_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental publication");
    let incremental_parser_cache = incremental_timings
        .parser_artifact_cache
        .as_ref()
        .expect("incremental parser cache telemetry");
    assert_eq!(
        incremental_parser_cache.policy,
        ArtifactCachePolicyDto::ReadThrough
    );
    assert_eq!(incremental_parser_cache.logical_lookups, 1);
    assert_eq!(incremental_parser_cache.physical_queries, 1);
    assert_eq!(incremental_parser_cache.hits, 0);
    assert_eq!(incremental_parser_cache.misses, 1);
    assert_eq!(incremental_parser_cache.reader_opens, 0);
    assert!(incremental_timings.full_refresh_queue_capacity.is_none());
    assert!(incremental_timings.full_refresh_chunks_produced.is_none());
    assert!(
        incremental_timings
            .full_refresh_chunk_target_bytes
            .is_none()
    );
    assert!(
        incremental_timings
            .full_refresh_chunk_target_nodes
            .is_none()
    );
    assert!(incremental_timings.full_refresh_chunk_planning_ms.is_none());
    assert!(incremental_timings.full_refresh_wall.is_none());
    let incremental_projection = incremental_timings
        .projection_persistence
        .as_ref()
        .expect("incremental projection persistence telemetry");
    assert_eq!(
        Some(incremental_projection.transactions),
        incremental_timings.projection_batch_transactions
    );
    assert!(incremental_projection.row_attempts > 0);
    assert_eq!(
        incremental_projection.dirty_state.statement_executions,
        u64::from(incremental_projection.transactions) * 4
    );
    assert!(
        incremental_timings
            .staged_sqlite_wal_autocheckpoint_bytes
            .is_none()
    );
    assert!(incremental_timings.staged_sqlite_checkpoint_ms.is_none());
    assert!(incremental_timings.staged_sqlite_sync_ms.is_none());
    let incremental_copy = incremental_timings
        .staged_snapshot_copy
        .as_ref()
        .expect("incremental snapshot-copy telemetry");
    assert!(incremental_copy.source_bytes > 0);
    assert_eq!(incremental_copy.source_bytes, incremental_copy.target_bytes);
    let incremental_promotion = incremental_timings
        .core_promotion
        .as_ref()
        .expect("incremental promotion telemetry");
    assert_eq!(
        incremental_promotion.previous_live_bytes,
        Some(incremental_copy.source_bytes)
    );
    assert_eq!(
        incremental_promotion.rollback_backup_bytes,
        incremental_promotion.previous_live_bytes
    );
    assert!(incremental_promotion.rollback_backup_copy_ms.is_some());
    assert!(incremental_promotion.backup_validation_ms.is_some());
    assert_promotion_reconciles(incremental_promotion);
    assert_eq!(incremental_timings.search_projection_rebuild_ms, Some(0));
    assert!(incremental_timings.search_symbol_stream_ms.is_some());
    assert!(
        incremental_timings
            .search_symbol_stream_rows
            .is_some_and(|rows| rows > full_timings.search_symbol_stream_rows.unwrap_or(0))
    );
    assert_eq!(incremental_timings.search_symbol_stream_batches, Some(1));
    let second = controller
        .index_publication()
        .expect("read second publication")
        .expect("second publication identity");
    assert_eq!(second.generation, 2);
    assert_eq!(second.mode, IndexPublicationMode::Incremental);
    assert_ne!(second.generation_id, first.generation_id);
    assert_ne!(second.run_id, first.run_id);

    let second_full_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("second full publication");
    assert!(second_full_timings.staged_snapshot_copy.is_none());
    let second_full_promotion = second_full_timings
        .core_promotion
        .as_ref()
        .expect("replacement full promotion telemetry");
    assert!(second_full_promotion.previous_live_bytes.is_some());
    assert!(second_full_promotion.rollback_backup_copy_ms.is_some());
    assert!(second_full_promotion.backup_validation_ms.is_some());
    assert_eq!(
        second_full_promotion.rollback_backup_bytes,
        second_full_promotion.previous_live_bytes
    );
    assert_promotion_reconciles(second_full_promotion);
    let third = controller
        .index_publication()
        .expect("read third publication")
        .expect("third publication identity");
    assert_eq!(third.generation, 3);
    assert_eq!(third.mode, IndexPublicationMode::Full);
    assert_ne!(third.generation_id, second.generation_id);
    assert_ne!(third.run_id, second.run_id);
    assert!(third.published_at_epoch_ms >= second.published_at_epoch_ms);
}

#[test]
fn structural_full_generations_reuse_unchanged_cache_and_preserve_previous_on_invalid_input() {
    let workspace = tempdir().expect("workspace dir");
    let markdown_path = workspace.path().join("guide.md");
    let json_path = workspace.path().join("config.json");
    fs::write(&markdown_path, "# Stable\n").expect("write markdown");
    fs::write(&json_path, "{\"service\":{\"name\":\"api\"}}\n").expect("write JSON");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");

    let first_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish first structural generation");
    let first_structural_cache = first_timings
        .structural_artifact_cache
        .as_ref()
        .expect("first structural cache telemetry");
    assert_eq!(
        first_structural_cache.policy,
        ArtifactCachePolicyDto::KnownEmpty
    );
    assert_eq!(first_structural_cache.logical_lookups, 2);
    assert_eq!(first_structural_cache.physical_queries, 0);
    assert_eq!(first_structural_cache.hits, 0);
    assert_eq!(first_structural_cache.misses, 2);
    assert_eq!(first_structural_cache.reader_opens, 0);
    assert_eq!(first_structural_cache.lookup_wall_ms, 0);
    let first = Store::database_index_publication(&storage_path)
        .expect("read first publication")
        .expect("first publication");
    let first_store = Store::open_read_only(&storage_path).expect("open first publication");
    let first_manifest = first_store
        .validate_structural_text_unit_publication(&first)
        .expect("validate first structural manifest");
    assert_eq!(first_manifest.projection_count, 2);
    assert!(first_manifest.unit_count >= 2);
    let first_cache = {
        let mut statement = first_store
            .get_connection()
            .prepare(
                "SELECT file_path, cache_key
             FROM structural_text_artifact_cache
             ORDER BY file_path",
            )
            .expect("prepare first structural cache keys");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query first structural cache keys")
            .map(|row| row.expect("read first structural cache key"))
            .collect::<HashMap<String, String>>()
    };
    drop(first_store);
    assert_eq!(first_cache.len(), 2);

    fs::write(&markdown_path, "# Replacement\n").expect("change markdown");
    let second_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish second structural generation");
    let second_structural_cache = second_timings
        .structural_artifact_cache
        .as_ref()
        .expect("second structural cache telemetry");
    assert_eq!(
        second_structural_cache.policy,
        ArtifactCachePolicyDto::ReadThrough
    );
    assert_eq!(second_structural_cache.logical_lookups, 2);
    assert_eq!(second_structural_cache.physical_queries, 2);
    assert_eq!(second_structural_cache.hits, 1);
    assert_eq!(second_structural_cache.misses, 1);
    assert_eq!(second_structural_cache.reader_opens, 1);
    let second = Store::database_index_publication(&storage_path)
        .expect("read second publication")
        .expect("second publication");
    assert_eq!(second.generation, first.generation + 1);
    let second_store = Store::open_read_only(&storage_path).expect("open second publication");
    let second_manifest = second_store
        .validate_structural_text_unit_publication(&second)
        .expect("validate second structural manifest");
    assert_eq!(second_manifest.projection_count, 2);
    assert_ne!(second_manifest.unit_digest, first_manifest.unit_digest);
    let second_cache = {
        let mut statement = second_store
            .get_connection()
            .prepare(
                "SELECT file_path, cache_key
             FROM structural_text_artifact_cache
             ORDER BY file_path",
            )
            .expect("prepare second structural cache keys");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query second structural cache keys")
            .map(|row| row.expect("read second structural cache key"))
            .collect::<HashMap<String, String>>()
    };
    drop(second_store);
    assert_ne!(first_cache.get("guide.md"), second_cache.get("guide.md"));
    assert_eq!(
        first_cache.get("config.json"),
        second_cache.get("config.json")
    );

    fs::write(&json_path, "{\"replacement\":{\"name\":\"api\"}}\n").expect("change JSON");
    std::fs::File::options()
        .write(true)
        .open(&json_path)
        .expect("open changed JSON")
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + Duration::from_secs(2)),
        )
        .expect("advance changed sql mtime");
    let incremental_timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("publish structural incremental");
    let incremental_structural_cache = incremental_timings
        .structural_artifact_cache
        .as_ref()
        .expect("incremental structural cache telemetry");
    assert_eq!(
        incremental_structural_cache.policy,
        ArtifactCachePolicyDto::ReadThrough
    );
    assert_eq!(incremental_structural_cache.logical_lookups, 1);
    assert_eq!(incremental_structural_cache.physical_queries, 1);
    assert_eq!(incremental_structural_cache.hits, 0);
    assert_eq!(incremental_structural_cache.misses, 1);
    assert_eq!(incremental_structural_cache.reader_opens, 0);
    let third = Store::database_index_publication(&storage_path)
        .expect("read incremental publication")
        .expect("incremental publication");
    assert_eq!(third.generation, second.generation + 1);
    assert_eq!(third.mode, IndexPublicationMode::Incremental);
    let third_store = Store::open_read_only(&storage_path).expect("open incremental");
    let third_manifest = third_store
        .validate_structural_text_unit_publication(&third)
        .expect("validate incremental structural manifest");
    assert_eq!(third_manifest.projection_count, 2);
    let third_names = third_store
        .get_nodes()
        .expect("read incremental nodes")
        .into_iter()
        .map(|node| node.serialized_name)
        .collect::<HashSet<_>>();
    assert!(
        third_names.contains("replacement"),
        "incremental structural nodes: {third_names:?}"
    );
    assert!(!third_names.contains("service"));
    drop(third_store);
    let structural_before_failure = structural_live_identity(&storage_path);
    assert!(structural_before_failure.manifest.unit_count > 0);
    assert_eq!(structural_before_failure.cache_rows.len(), 2);

    fs::write(&markdown_path, [0xff, 0xfe, 0xfd]).expect("write invalid utf8");
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("invalid structural input must fail closed");
    assert_eq!(error.code, "source_binary");
    assert!(error.details.as_ref().is_some_and(|details| {
        details
            .coverage_gaps
            .iter()
            .any(|gap| gap.reason == FileCoverageReason::Binary)
    }));
    let preserved = Store::database_index_publication(&storage_path)
        .expect("read preserved publication")
        .expect("preserved publication");
    assert_eq!(preserved, third);
    Store::open_read_only(&storage_path)
        .expect("open preserved publication")
        .validate_structural_text_unit_publication(&preserved)
        .expect("previous structural manifest remains valid");
    assert_eq!(
        structural_live_identity(&storage_path),
        structural_before_failure,
        "collector failure changed the prior structural manifest or cache identity"
    );
    assert_no_staged_publication_artifacts(&storage_path);

    fs::write(&markdown_path, "# Replacement\n").expect("restore markdown");
    fs::write(
        workspace.path().join("malformed.json"),
        "{\"missing_value\":",
    )
    .expect("write malformed JSON");
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("malformed structural input must fail closed");
    assert_eq!(error.code, "source_malformed");
    assert!(error.details.as_ref().is_some_and(|details| {
        details
            .coverage_gaps
            .iter()
            .any(|gap| gap.reason == FileCoverageReason::Malformed)
    }));
    assert_eq!(
        structural_live_identity(&storage_path),
        structural_before_failure,
        "malformed input changed the prior structural manifest or cache identity"
    );
    assert_no_staged_publication_artifacts(&storage_path);

    fs::remove_file(workspace.path().join("malformed.json"))
        .expect("remove malformed JSON fixture");
    let unreadable_path = markdown_path.clone();
    arm_source_policy_after_plan_hook(move || {
        fs::remove_file(&unreadable_path).expect("remove planned markdown source");
        fs::create_dir(&unreadable_path)
            .expect("replace planned markdown source with unreadable directory");
    });
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("unreadable structural replacement must fail closed");
    assert_eq!(error.code, "source_unreadable");
    assert!(error.details.as_ref().is_some_and(|details| {
        details
            .coverage_gaps
            .iter()
            .any(|gap| gap.reason == FileCoverageReason::Unreadable)
    }));
    assert_eq!(
        structural_live_identity(&storage_path),
        structural_before_failure,
        "unreadable replacement changed the prior core publication, structural manifest, or cache identity"
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn structural_publication_survives_unreadable_and_partial_discovery_failures() {
    for scenario in ["unreadable", "partial-discovery"] {
        let workspace = tempdir().expect("workspace dir");
        let source_root = workspace.path().join("src");
        fs::create_dir_all(&source_root).expect("source directory");
        let css_path = source_root.join("styles.css");
        fs::write(&css_path, ".stable { color: green; }\n").expect("write structural source");
        let manifest_path = workspace.path().join("codestory_workspace.json");
        fs::write(&manifest_path, r#"{"members":["src"]}"#)
            .expect("write complete workspace manifest");
        let storage_path = workspace.path().join(".cache/codestory.db");
        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open structural project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish structural baseline");
        let baseline = structural_live_identity(&storage_path);
        assert!(baseline.manifest.unit_count > 0);
        assert_eq!(baseline.cache_rows.len(), 1);

        let error = if scenario == "unreadable" {
            let unreadable_path = css_path.clone();
            arm_source_policy_after_plan_hook(move || {
                fs::remove_file(&unreadable_path).expect("remove planned structural source");
                fs::create_dir(&unreadable_path)
                    .expect("replace planned source with unreadable directory");
            });
            controller
                .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
                .expect_err("unreadable structural source must reject publication")
        } else {
            fs::write(&manifest_path, r#"{"members":["src","missing-member"]}"#)
                .expect("make workspace discovery partial");
            controller
                .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
                .expect_err("partial discovery must reject publication")
        };
        let expected_reason = if scenario == "unreadable" {
            FileCoverageReason::Unreadable
        } else {
            FileCoverageReason::DiscoveryIncomplete
        };
        assert!(
            error.details.as_ref().is_some_and(|details| details
                .coverage_gaps
                .iter()
                .any(|gap| gap.reason == expected_reason)),
            "{scenario} did not report the expected coverage gap: {error:?}"
        );
        assert_eq!(
            structural_live_identity(&storage_path),
            baseline,
            "{scenario} changed the prior structural manifest or cache identity"
        );
        assert_no_staged_publication_artifacts(&storage_path);
    }
}

#[test]
fn staged_structural_cache_write_failure_preserves_nonempty_live_generation() {
    let workspace = tempdir().expect("workspace dir");
    let css_path = workspace.path().join("styles.css");
    fs::write(&css_path, ".stable { color: green; }\n").expect("write structural source");
    let storage_path = workspace.path().join(".cache/codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open structural project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish structural baseline");
    let baseline = structural_live_identity(&storage_path);
    assert!(baseline.manifest.unit_count > 0);
    assert_eq!(baseline.cache_rows.len(), 1);

    fs::write(&css_path, ".replacement { color: blue; }\n")
        .expect("write replacement structural source");
    arm_full_refresh_staged_store_hook(|storage| {
        storage
            .get_connection()
            .execute_batch(
                "CREATE TRIGGER reject_structural_cache_write
                 BEFORE INSERT ON structural_text_artifact_cache
                 BEGIN
                   SELECT RAISE(ABORT, 'forced staged structural cache failure');
                 END;",
            )
            .expect("install staged structural cache fault");
    });
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("staged structural write failure must reject publication");
    assert!(
        error
            .message
            .contains("forced staged structural cache failure"),
        "{error:?}"
    );
    assert_eq!(
        structural_live_identity(&storage_path),
        baseline,
        "staged structural write failure changed the live manifest or cache identity"
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn incremental_cache_read_faults_recollect_in_staged_candidate_and_preserve_live_publication() {
    for (family, cache_table) in [
        ("parser", "index_artifact_cache"),
        ("structural", "structural_text_artifact_cache"),
    ] {
        let workspace = tempdir().expect("workspace dir");
        let rust_path = workspace.path().join("lib.rs");
        let json_path = workspace.path().join("config.json");
        fs::write(&rust_path, "pub fn retained_parser() -> i32 { 1 }\n")
            .expect("write parser source");
        fs::write(&json_path, "{\"service\":{\"name\":\"api\"}}\n")
            .expect("write structural source");
        let storage_path = workspace.path().join(".cache/codestory.db");
        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open cache fault project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish cache fault baseline");

        let parser_baseline = parser_cache_live_identity(&storage_path);
        let structural_baseline = structural_live_identity(&storage_path);
        assert_eq!(parser_baseline.cache_rows.len(), 1);
        assert_eq!(structural_baseline.cache_rows.len(), 1);
        assert_eq!(
            parser_baseline.publication, structural_baseline.publication,
            "cache families must belong to the same complete publication"
        );

        let changed_path = if family == "parser" {
            fs::write(&rust_path, "pub fn replacement_parser() -> i32 { 2 }\n")
                .expect("change parser source");
            &rust_path
        } else {
            fs::write(&json_path, "{\"replacement\":{\"name\":\"api\"}}\n")
                .expect("change structural source");
            &json_path
        };
        std::fs::File::options()
            .write(true)
            .open(changed_path)
            .expect("open changed cache source")
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() + Duration::from_secs(2)),
            )
            .expect("advance changed cache source mtime");

        let denied_reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let staged_cache_writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let denied_reads_hook = denied_reads.clone();
        let staged_cache_writes_hook = staged_cache_writes.clone();
        arm_incremental_staged_store_hook(move |storage| {
            let denied_reads = denied_reads_hook.clone();
            storage
                .get_connection()
                .authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                    if matches!(
                        context.action,
                        rusqlite::hooks::AuthAction::Read { table_name, .. }
                            if table_name == cache_table
                    ) && denied_reads
                        .compare_exchange(
                            0,
                            1,
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                        )
                        .is_ok()
                    {
                        rusqlite::hooks::Authorization::Deny
                    } else {
                        rusqlite::hooks::Authorization::Allow
                    }
                }))
                .expect("install staged cache read fault");
            storage
                .get_connection()
                .update_hook(Some(move |action, _: &str, updated_table: &str, _| {
                    if updated_table == cache_table
                        && matches!(
                            action,
                            rusqlite::hooks::Action::SQLITE_INSERT
                                | rusqlite::hooks::Action::SQLITE_UPDATE
                        )
                    {
                        staged_cache_writes_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }))
                .expect("observe staged cache writes");
        });
        arm_publication_test_fault(
            PublicationTestBoundary::SearchBuild,
            PublicationTestAction::Fail,
        );

        let error = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect_err("later publication fault must reject recollected candidate");
        assert!(
            error.message.contains("SearchBuild"),
            "{family} candidate did not reach the later publication boundary: {error:?}"
        );
        assert_eq!(
            denied_reads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{family} staged cache read fault was not exercised exactly once"
        );
        assert!(
            staged_cache_writes.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "{family} read failure did not recollect and persist a staged cache entry"
        );
        PUBLICATION_TEST_FAULT.with(|fault| {
            assert!(
                fault.borrow().is_none(),
                "{family} candidate did not reach the armed publication fault"
            );
        });

        assert_eq!(
            parser_cache_live_identity(&storage_path),
            parser_baseline,
            "{family} candidate changed the live parser cache or publication"
        );
        assert_eq!(
            structural_live_identity(&storage_path),
            structural_baseline,
            "{family} candidate changed the live structural cache, manifest, or publication"
        );
        let live = Storage::open_read_only(&storage_path).expect("open preserved live publication");
        assert!(storage_has_symbol(&live, "retained_parser"));
        assert!(storage_has_symbol(&live, "service"));
        assert!(!storage_has_symbol(&live, "replacement_parser"));
        assert!(!storage_has_symbol(&live, "replacement"));
        drop(live);
        assert_no_staged_publication_artifacts(&storage_path);
    }
}

#[test]
fn structural_publication_survives_cancellation_and_promotion_rollback_boundaries() {
    for (boundary, action, mode) in [
        (
            PublicationTestBoundary::SearchBuild,
            PublicationTestAction::Cancel,
            IndexMode::Full,
        ),
        (
            PublicationTestBoundary::DatabaseReplacement,
            PublicationTestAction::Fail,
            IndexMode::Full,
        ),
        (
            PublicationTestBoundary::MarkerCompletion,
            PublicationTestAction::Fail,
            IndexMode::Incremental,
        ),
    ] {
        let workspace = tempdir().expect("workspace dir");
        let css_path = workspace.path().join("styles.css");
        let rust_path = workspace.path().join("lib.rs");
        fs::write(&css_path, ".stable { color: green; }\n").expect("write structural source");
        fs::write(&rust_path, "pub fn baseline() -> i32 { 1 }\n").expect("write parser source");
        let storage_path = workspace.path().join(".cache/codestory.db");
        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open structural project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish structural baseline");
        let baseline = structural_live_identity(&storage_path);
        assert!(baseline.manifest.unit_count > 0);
        assert_eq!(baseline.cache_rows.len(), 1);

        fs::write(&rust_path, "pub fn replacement() -> i32 { 2 }\n")
            .expect("write replacement parser source");
        let cancel_token = CancellationToken::new();
        arm_publication_test_fault(boundary, action);
        let error = controller
            .run_indexing_blocking_without_runtime_refresh_with_cancel(mode, &cancel_token)
            .expect_err("injected transition must reject publication");
        PUBLICATION_TEST_FAULT.with(|fault| {
            assert!(
                fault.borrow().is_none(),
                "structural transition fault was not reached: {boundary:?}"
            );
        });
        match action {
            PublicationTestAction::Cancel => {
                assert_eq!(error.code, "cancelled");
                assert!(cancel_token.is_cancelled());
            }
            PublicationTestAction::Fail => {
                assert_eq!(error.code, "internal");
                assert!(error.message.contains(&format!("{boundary:?}")));
            }
        }
        assert_eq!(
            structural_live_identity(&storage_path),
            baseline,
            "{boundary:?} changed the prior structural manifest or cache identity"
        );
        assert_no_staged_publication_artifacts(&storage_path);
    }
}

#[test]
fn explicit_incremental_rejects_incompatible_structural_publication_before_source_reads() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"legacy-demo\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline");
    let previous = Store::database_index_publication(&storage_path)
        .expect("read baseline")
        .expect("baseline publication");
    let storage = Store::open(&storage_path).expect("open baseline");
    storage
        .get_connection()
        .execute("DELETE FROM structural_text_unit_publication", [])
        .expect("remove structural manifest");
    drop(storage);
    fs::write(
        workspace.path().join("malformed.json"),
        "{\"missing_value\":",
    )
    .expect("write source that would fail a full parse");
    let database_before = fs::read(&storage_path).expect("read incompatible database");
    let wal_path = storage_path.with_extension("db-wal");
    let wal_before = fs::read(&wal_path).ok();

    for error in [
        controller
            .dry_run_index(IndexMode::Incremental)
            .expect_err("dry-run must reject incompatible incremental"),
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect_err("execution must reject incompatible incremental"),
    ] {
        assert_eq!(error.code, FULL_REFRESH_REQUIRED_ERROR_CODE);
        assert!(error.message.contains("requested=incremental"));
        assert!(error.message.contains("effective=none"));
        assert!(error.message.contains("required=full"));
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|details| details.cause_code.as_deref()),
            Some("structural_publication_incompatible")
        );
    }
    assert_eq!(
        fs::read(&storage_path).expect("read database after rejected requests"),
        database_before,
        "compatibility rejection must not mutate the live database"
    );
    assert_eq!(
        fs::read(&wal_path).ok(),
        wal_before,
        "compatibility rejection must not mutate the live WAL"
    );
    assert_eq!(
        Store::database_index_publication(&storage_path)
            .expect("read preserved publication")
            .expect("preserved publication"),
        previous
    );
    assert!(!controller.state.lock().is_indexing);
    assert_no_staged_publication_artifacts(&storage_path);

    let full_error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("explicit full refresh must reach malformed source verification");
    assert_eq!(full_error.code, "source_malformed");
    assert!(
        full_error
            .message
            .contains("Effective refresh mode `full` could not verify"),
        "unexpected full-refresh error: {full_error:?}"
    );
}

#[test]
fn precurrent_schema_requires_typed_full_without_mutating_database_or_sidecars() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn legacy_value() -> i32 { 28 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish current baseline");
    {
        let storage = Storage::open(&storage_path).expect("open schema fixture");
        storage
            .get_connection()
            .pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION - 1)
            .expect("stamp supported pre-current schema");
    }
    let database_before = fs::read(&storage_path).expect("read old-schema database");
    let wal_path = storage_path.with_extension("db-wal");
    let wal_before = fs::read(&wal_path).ok();
    let cache_path = storage_path.parent().expect("cache path");
    let cache_entries_before = {
        let mut entries = fs::read_dir(cache_path)
            .expect("list cache before compatibility checks")
            .map(|entry| {
                entry
                    .expect("read cache entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries
    };

    for error in [
        controller
            .dry_run_index(IndexMode::Incremental)
            .expect_err("dry-run must reject the old schema"),
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect_err("execution must reject the old schema"),
    ] {
        assert_eq!(error.code, FULL_REFRESH_REQUIRED_ERROR_CODE);
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|details| details.cause_code.as_deref()),
            Some("core_schema_upgrade_required")
        );
    }
    let auto_effective_dry_run = controller
        .dry_run_index(IndexMode::Full)
        .expect("auto-selected full dry-run must inspect old schema without migrating it");
    assert_eq!(auto_effective_dry_run.refresh, IndexMode::Full);
    assert_eq!(
        fs::read(&storage_path).expect("read database after rejected requests"),
        database_before,
        "old-schema compatibility checks must preserve database bytes"
    );
    assert_eq!(
        fs::read(&wal_path).ok(),
        wal_before,
        "old-schema compatibility checks must preserve WAL bytes"
    );
    let cache_entries_after = {
        let mut entries = fs::read_dir(cache_path)
            .expect("list cache after compatibility checks")
            .map(|entry| {
                entry
                    .expect("read cache entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries
    };
    assert_eq!(cache_entries_after, cache_entries_before);
    assert!(!controller.state.lock().is_indexing);
    assert_no_staged_publication_artifacts(&storage_path);

    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("explicit full refresh upgrades the supported old schema");
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("read upgraded schema"),
        CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn full_refresh_required_command_quotes_shell_metacharacters() {
    let error = full_refresh_required_error(
        Path::new("repo/$hidden/quoted'path"),
        "core_schema_upgrade_required",
        "core_schema_upgrade_required",
    );
    let command = error
        .details
        .expect("typed compatibility details")
        .next_commands
        .into_iter()
        .next()
        .expect("full-refresh repair command");

    #[cfg(not(windows))]
    assert_eq!(
        command,
        "codestory-cli index --project 'repo/$hidden/quoted'\\''path' --refresh full"
    );
    #[cfg(windows)]
    assert_eq!(
        command,
        "codestory-cli index --project 'repo/$hidden/quoted''path' --refresh full"
    );
}

#[test]
fn full_refresh_pipeline_writer_failure_preserves_live_publication() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn retained_value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline");
    let baseline = Storage::open(&storage_path)
        .expect("open baseline")
        .get_complete_index_publication()
        .expect("read baseline publication")
        .expect("baseline publication");

    arm_full_refresh_staged_store_hook(|storage| {
        storage
            .get_connection()
            .execute_batch(
                "CREATE TRIGGER reject_pipeline_cache_write
                 BEFORE INSERT ON index_artifact_cache
                 BEGIN
                   SELECT RAISE(ABORT, 'forced runtime pipeline cache failure');
                 END;",
            )
            .expect("install staged writer failure");
    });
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("injected pipeline writer failure must reject the candidate");
    assert!(
        error
            .message
            .contains("forced runtime pipeline cache failure"),
        "{error:?}"
    );

    let live = Storage::open(&storage_path).expect("reopen retained live publication");
    assert_eq!(
        live.get_complete_index_publication()
            .expect("read retained publication"),
        Some(baseline)
    );
    assert!(
        live.get_nodes()
            .expect("read retained graph")
            .iter()
            .any(|node| node.serialized_name == "retained_value")
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn full_refresh_semantic_endpoint_index_failure_preserves_live_publication() {
    let workspace = tempdir().expect("workspace dir");
    let source_path = workspace.path().join("lib.rs");
    fs::write(&source_path, "pub fn retained_generation() -> i32 { 1 }\n")
        .expect("write baseline source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline");
    let baseline = Storage::open(&storage_path)
        .expect("open baseline")
        .get_complete_index_publication()
        .expect("read baseline publication")
        .expect("baseline publication");

    fs::write(&source_path, "pub fn rejected_generation() -> i32 { 2 }\n")
        .expect("write replacement source");
    arm_full_refresh_staged_store_hook(|storage| {
        storage
            .get_connection()
            .execute_batch(
                "CREATE TABLE idx_edge_source (
                     collision INTEGER
                 );",
            )
            .expect("install semantic endpoint index collision");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("semantic endpoint index failure must reject the candidate");
    assert!(error.message.contains("idx_edge_source"), "{error:?}");

    let live = Storage::open(&storage_path).expect("reopen retained live publication");
    assert_eq!(
        live.get_complete_index_publication()
            .expect("read retained publication"),
        Some(baseline)
    );
    assert!(storage_has_symbol(&live, "retained_generation"));
    assert!(!storage_has_symbol(&live, "rejected_generation"));
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn full_refresh_post_summary_index_failure_preserves_live_publication() {
    let workspace = tempdir().expect("workspace dir");
    let source_path = workspace.path().join("lib.rs");
    fs::write(&source_path, "pub fn retained_generation() -> i32 { 1 }\n")
        .expect("write baseline source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish baseline");
    let baseline = Storage::open(&storage_path)
        .expect("open baseline")
        .get_complete_index_publication()
        .expect("read baseline publication")
        .expect("baseline publication");

    fs::write(&source_path, "pub fn rejected_generation() -> i32 { 2 }\n")
        .expect("write replacement source");
    arm_full_refresh_staged_store_hook(|storage| {
        storage
            .get_connection()
            .execute_batch(
                "CREATE TABLE idx_grounding_file_snapshot_path (
                     collision INTEGER
                 );",
            )
            .expect("install post-summary index collision");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("post-summary destination index failure must reject the candidate");
    assert!(
        error.message.contains("idx_grounding_file_snapshot_path"),
        "{error:?}"
    );

    let live = Storage::open(&storage_path).expect("reopen retained live publication");
    assert_eq!(
        live.get_complete_index_publication()
            .expect("read retained publication"),
        Some(baseline)
    );
    assert!(storage_has_symbol(&live, "retained_generation"));
    assert!(!storage_has_symbol(&live, "rejected_generation"));
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn incremental_publication_ignores_changed_files_without_graph_collectors() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn first_value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let unsupported = workspace.path().join("notes.txt");
    fs::write(&unsupported, "Initial notes\n").expect("write unsupported file");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");

    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("first full publication");
    let first = controller
        .index_publication()
        .expect("read first publication")
        .expect("first publication identity");

    fs::write(&unsupported, "Updated notes\n").expect("update unsupported file");
    let dry_run = controller
        .dry_run_index(IndexMode::Incremental)
        .expect("plan unsupported file refresh");
    assert!(
        dry_run
            .sample_files_to_index
            .iter()
            .any(|path| path == "notes.txt"),
        "the regression must exercise a discovered file in the refresh plan: {dry_run:?}"
    );
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("incremental publication after unsupported file change");
    let second = controller
        .index_publication()
        .expect("read second publication")
        .expect("second publication identity");

    assert_eq!(second.generation, first.generation + 1);
    assert_eq!(second.mode, IndexPublicationMode::Incremental);
    assert!(
        Storage::open(&storage_path)
            .expect("open published storage")
            .get_file_by_path(&unsupported)
            .expect("look up unsupported file")
            .is_none(),
        "files without graph collectors should not be invented in semantic scope"
    );
}

#[test]
fn incomplete_legacy_run_is_not_a_servable_complete_publication() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(workspace.path().join("lib.rs"), "pub fn value() {}\n").expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete generation");
    assert!(
        controller
            .complete_index_publication()
            .expect("read complete publication")
            .is_some()
    );

    Storage::open(&storage_path)
        .expect("open live storage")
        .begin_incremental_run()
        .expect("mark legacy incomplete run");

    assert!(
        controller
            .complete_index_publication()
            .expect("read fenced publication")
            .is_none()
    );
}

#[test]
fn legacy_schema_18_incomplete_marker_requires_explicit_full_recovery() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn legacy_value() -> i32 { 18 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    {
        let storage = Storage::open(&storage_path).expect("open legacy seed storage");
        storage
            .get_connection()
            .execute_batch("DROP TABLE index_publication;")
            .expect("remove post-v18 publication table");
        storage
            .get_connection()
            .pragma_update(None, "user_version", 18)
            .expect("stamp schema 18");
        storage
            .begin_incremental_run()
            .expect("install legacy incomplete marker");
    }

    assert!(
        Storage::database_index_publication(&storage_path)
            .expect("read legacy publication")
            .is_none()
    );
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect_err("explicit incremental must reject the legacy incomplete marker");
    assert_eq!(error.code, FULL_REFRESH_REQUIRED_ERROR_CODE);
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.cause_code.as_deref()),
        Some("incomplete_incremental_publication")
    );
    assert!(
        Storage::database_index_publication(&storage_path)
            .expect("read rejected legacy publication")
            .is_none()
    );

    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("recover legacy marker with an explicit full refresh");

    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("recovered schema"),
        codestory_store::CURRENT_SCHEMA_VERSION
    );
    let publication = controller
        .index_publication()
        .expect("read recovered publication")
        .expect("recovered publication identity");
    assert_eq!(publication.generation, 1);
    assert_eq!(publication.mode, IndexPublicationMode::Full);
}

#[test]
fn staged_recovery_search_failure_preserves_the_marked_live_database() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");
    Storage::open(&storage_path)
        .expect("open storage")
        .begin_incremental_run()
        .expect("simulate interrupted incremental");

    let search_path = search_index_generation_root(&storage_path);
    if search_path.is_dir() {
        fs::remove_dir_all(&search_path).expect("remove search directory");
    }
    fs::write(&search_path, b"not a search directory").expect("block search rebuild path");

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("explicit full recovery cache rebuild must fail");

    assert!(
        error.message.contains("search"),
        "unexpected error: {error:?}"
    );
    let storage = Storage::open(&storage_path).expect("open replacement database");
    assert!(
        storage
            .has_incomplete_incremental_run()
            .expect("replacement marker")
    );
    assert!(
        storage
            .snapshots()
            .has_ready_summary()
            .expect("live summary readiness"),
        "pre-publication failure must preserve the live summary snapshot"
    );
    assert!(
        storage
            .snapshots()
            .has_ready_detail()
            .expect("live detail readiness"),
        "pre-publication failure must preserve the live detail snapshot"
    );
    storage
        .get_connection()
        .execute(
            "UPDATE incomplete_index_run
             SET started_at_epoch_ms = started_at_epoch_ms
             WHERE id = 1",
            [],
        )
        .expect("retain the fence in committed WAL state");
    let fenced_schema =
        Storage::database_schema_version(&storage_path).expect("replacement schema");
    assert_ne!(fenced_schema, codestory_store::CURRENT_SCHEMA_VERSION);
    let wal_path = storage_path.with_extension("db-wal");
    assert!(wal_path.is_file(), "fenced fixture must retain WAL state");
    let database_before = fs::read(&storage_path).expect("read fenced database before freshness");
    let wal_before = fs::read(&wal_path).expect("read fenced WAL before freshness");

    let cached = controller
        .index_freshness()
        .expect("cached recovery freshness");
    let uncached = controller
        .index_freshness_uncached()
        .expect("uncached recovery freshness");
    for freshness in [&cached, &uncached] {
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
    assert_eq!(
        fs::read(&storage_path).expect("read fenced database after freshness"),
        database_before
    );
    assert_eq!(
        fs::read(&wal_path).expect("read fenced WAL after freshness"),
        wal_before
    );
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("schema after freshness"),
        fenced_schema
    );
    assert!(
        storage
            .has_incomplete_incremental_run()
            .expect("marker after freshness"),
        "freshness observation must preserve the durable fence"
    );

    drop(storage);
    fs::remove_file(&search_path).expect("remove search rebuild blocker");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("successful full recovery");
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("recovered schema"),
        codestory_store::CURRENT_SCHEMA_VERSION
    );
    let readable = Storage::open_read_only(&storage_path).expect("open recovered publication");
    assert!(
        !readable
            .has_incomplete_incremental_run()
            .expect("read recovered marker")
    );
    assert!(
        readable
            .get_complete_index_publication()
            .expect("read recovered publication")
            .is_some()
    );
}

#[test]
fn successful_index_reopen_failure_does_not_leave_indexing_stuck() {
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("missing").join("codestory.db");
    let controller = AppController::new();

    {
        let mut state = controller.state.lock();
        state.is_indexing = true;
        state
            .node_names
            .insert(CoreNodeId(999), "stale_symbol".to_string());
        let engine = SearchEngine::new(None).expect("search engine");
        publish_search_engine(&mut state, engine, None);
    }

    let error = controller
        .finish_successful_indexing(empty_indexing_run_summary(), &storage_path, true, None)
        .expect_err("storage reopen failure should propagate");

    assert_eq!(error.code, "internal");
    assert!(error.message.contains("Failed to reopen storage"));

    let state = controller.state.lock();
    assert!(!state.is_indexing);
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn blocking_index_without_open_project_does_not_leave_indexing_stuck() {
    let controller = AppController::new();

    let error = controller
        .run_indexing_blocking(IndexMode::Full)
        .expect_err("missing project should error");

    assert_eq!(error.code, "invalid_argument");
    assert!(!controller.state.lock().is_indexing);
}

#[derive(Debug, Clone, Copy)]
enum IncrementalFailureBoundary {
    Projection,
    Cleanup,
    Resolution,
    SemanticDocs,
    SummarySnapshot,
    DetailSnapshot,
    PublicationIdentity,
    MarkerClear,
}

fn incremental_failure_trigger(boundary: IncrementalFailureBoundary) -> &'static str {
    match boundary {
        IncrementalFailureBoundary::Projection => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE INSERT ON file
             BEGIN SELECT RAISE(ABORT, 'forced projection failure'); END;"
        }
        IncrementalFailureBoundary::Cleanup => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE DELETE ON file
             BEGIN SELECT RAISE(ABORT, 'forced cleanup failure'); END;"
        }
        IncrementalFailureBoundary::Resolution => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE UPDATE OF resolved_source_node_id ON edge
             WHEN NEW.resolved_source_node_id IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'forced resolution failure'); END;"
        }
        IncrementalFailureBoundary::SemanticDocs => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE INSERT ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'forced semantic doc failure'); END;"
        }
        IncrementalFailureBoundary::SummarySnapshot => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE INSERT ON grounding_file_snapshot
             BEGIN SELECT RAISE(ABORT, 'forced summary snapshot failure'); END;"
        }
        IncrementalFailureBoundary::DetailSnapshot => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE INSERT ON grounding_node_snapshot
             BEGIN SELECT RAISE(ABORT, 'forced detail snapshot failure'); END;"
        }
        IncrementalFailureBoundary::PublicationIdentity => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE INSERT ON index_publication
             BEGIN SELECT RAISE(ABORT, 'forced publication identity failure'); END;"
        }
        IncrementalFailureBoundary::MarkerClear => {
            "CREATE TRIGGER fail_incremental_boundary
             BEFORE DELETE ON incomplete_index_run
             BEGIN SELECT RAISE(ABORT, 'forced marker clear failure'); END;"
        }
    }
}

fn incremental_failure_message(boundary: IncrementalFailureBoundary) -> &'static str {
    match boundary {
        IncrementalFailureBoundary::Projection => "forced projection failure",
        IncrementalFailureBoundary::Cleanup => "forced cleanup failure",
        IncrementalFailureBoundary::Resolution => "forced resolution failure",
        IncrementalFailureBoundary::SemanticDocs => "forced semantic doc failure",
        IncrementalFailureBoundary::SummarySnapshot => "forced summary snapshot failure",
        IncrementalFailureBoundary::DetailSnapshot => "forced detail snapshot failure",
        IncrementalFailureBoundary::PublicationIdentity => "forced publication identity failure",
        IncrementalFailureBoundary::MarkerClear => "forced marker clear failure",
    }
}

fn assert_incremental_boundary_is_atomic(boundary: IncrementalFailureBoundary) {
    let workspace = tempdir().expect("workspace dir");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let old_path = workspace.path().join("old.rs");
    let new_path = workspace.path().join("new.rs");
    fs::write(&old_path, "pub fn old_value() -> i32 { 1 }\n").expect("write old source");

    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let (
        baseline_paths,
        baseline_stats,
        baseline_snapshots,
        baseline_schema,
        baseline_publication,
        baseline_search_generations,
        baseline_semantic_docs,
        baseline_symbol_doc_count,
    ) = {
        let storage = Storage::open(&storage_path).expect("open baseline storage");
        let paths = storage
            .get_files()
            .expect("read baseline files")
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        let stats = storage.get_stats().expect("read baseline stats");
        let snapshots = storage
            .snapshots()
            .get_metadata()
            .expect("read baseline snapshot metadata");
        let schema =
            Storage::database_schema_version(&storage_path).expect("read baseline schema version");
        let publication = storage
            .get_index_publication()
            .expect("read baseline publication")
            .expect("baseline publication identity");
        let search_generations = persisted_search_generation_names(&storage_path);
        let semantic_docs = storage
            .get_all_llm_symbol_docs()
            .expect("read baseline semantic docs");
        let symbol_doc_count = storage
            .get_symbol_search_doc_count()
            .expect("read baseline symbol doc count");
        (
            paths,
            stats,
            snapshots,
            schema,
            publication,
            search_generations,
            semantic_docs,
            symbol_doc_count,
        )
    };

    fs::remove_file(&old_path).expect("remove old source");
    fs::write(
        &new_path,
        "pub fn caller() -> i32 { target() }\npub fn target() -> i32 { 2 }\n",
    )
    .expect("write new source");
    {
        let storage = Storage::open(&storage_path).expect("open storage for fault trigger");
        storage
            .get_connection()
            .execute_batch(incremental_failure_trigger(boundary))
            .expect("install fault trigger");
    }

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect_err("incremental boundary fault must fail the run");
    assert_eq!(error.code, "internal", "boundary={boundary:?}: {error:?}");
    assert!(
        error
            .message
            .contains(incremental_failure_message(boundary)),
        "wrong failure boundary for {boundary:?}: {error:?}"
    );

    let storage = Storage::open(&storage_path).expect("reopen live storage");
    assert!(
        !storage
            .has_incomplete_incremental_run()
            .expect("read live marker"),
        "pre-publish failure must not mark live storage: {boundary:?}"
    );
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("live schema version"),
        baseline_schema,
        "pre-publish failure must not change the live schema: {boundary:?}"
    );
    let live_paths = storage
        .get_files()
        .expect("read live files")
        .into_iter()
        .map(|file| file.path)
        .collect::<Vec<_>>();
    assert_eq!(live_paths, baseline_paths, "boundary={boundary:?}");
    let live_stats = storage.get_stats().expect("read live stats");
    assert_eq!(live_stats.node_count, baseline_stats.node_count);
    assert_eq!(live_stats.edge_count, baseline_stats.edge_count);
    assert_eq!(live_stats.file_count, baseline_stats.file_count);
    assert_eq!(live_stats.error_count, baseline_stats.error_count);
    assert_eq!(
        storage
            .snapshots()
            .get_metadata()
            .expect("read live snapshot metadata"),
        baseline_snapshots,
        "pre-publish failure must preserve the complete old snapshot generation"
    );
    assert_eq!(
        storage
            .get_index_publication()
            .expect("read live publication identity"),
        Some(baseline_publication.clone()),
        "pre-publish failure must not advance the live generation"
    );
    assert_eq!(
        persisted_search_generation_names(&storage_path),
        baseline_search_generations,
        "pre-publish failure must not create search state for an unpublished generation"
    );
    assert_eq!(
        storage
            .get_all_llm_symbol_docs()
            .expect("read live semantic docs"),
        baseline_semantic_docs,
        "pre-publish failure must preserve the live semantic corpus"
    );
    assert_eq!(
        storage
            .get_symbol_search_doc_count()
            .expect("read live symbol doc count"),
        baseline_symbol_doc_count,
        "pre-publish failure must preserve graph-native semantic docs"
    );
    storage
        .get_connection()
        .execute_batch("DROP TRIGGER fail_incremental_boundary;")
        .expect("remove injected live trigger");
    drop(storage);

    let dry_run = controller
        .dry_run_index(IndexMode::Incremental)
        .expect("dry-run direct retry");
    assert_eq!(dry_run.refresh, IndexMode::Incremental);
    let timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("direct incremental retry");
    assert!(timings.publish_ms.is_some());
    let storage = Storage::open(&storage_path).expect("open retried storage");
    assert!(
        storage
            .get_file_by_path(&old_path)
            .expect("read old file after retry")
            .is_none()
    );
    assert!(
        storage
            .get_file_by_path(&new_path)
            .expect("read new file after retry")
            .is_some()
    );
    assert!(
        !storage
            .has_incomplete_incremental_run()
            .expect("marker after direct retry")
    );
    let retried_publication = storage
        .get_index_publication()
        .expect("read retried publication")
        .expect("retried publication identity");
    assert_eq!(
        retried_publication.generation,
        baseline_publication.generation + 1
    );
    assert_eq!(retried_publication.mode, IndexPublicationMode::Incremental);
}

#[test]
fn incremental_boundaries_preserve_live_state_and_retry_atomically() {
    for boundary in [
        IncrementalFailureBoundary::Projection,
        IncrementalFailureBoundary::Cleanup,
        IncrementalFailureBoundary::Resolution,
        IncrementalFailureBoundary::SemanticDocs,
        IncrementalFailureBoundary::SummarySnapshot,
        IncrementalFailureBoundary::DetailSnapshot,
        IncrementalFailureBoundary::PublicationIdentity,
        IncrementalFailureBoundary::MarkerClear,
    ] {
        assert_incremental_boundary_is_atomic(boundary);
    }
}

const PUBLICATION_TRANSITION_BOUNDARIES: [PublicationTestBoundary; 13] = [
    PublicationTestBoundary::SemanticContextIndexes,
    PublicationTestBoundary::SemanticNodePage,
    PublicationTestBoundary::SemanticEndpointRead,
    PublicationTestBoundary::Identity,
    PublicationTestBoundary::SearchBuild,
    PublicationTestBoundary::SearchSymbolPage,
    PublicationTestBoundary::SearchIndexWrite,
    PublicationTestBoundary::SearchValidation,
    PublicationTestBoundary::SearchCompletion,
    PublicationTestBoundary::CatalogLock,
    PublicationTestBoundary::DatabaseReplacement,
    PublicationTestBoundary::MarkerCompletion,
    PublicationTestBoundary::RuntimeCache,
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct StructuralLiveIdentity {
    publication: IndexPublicationRecord,
    manifest: codestory_store::StructuralTextUnitPublicationManifest,
    cache_rows: Vec<(String, i64, String, String, i64, String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParserCacheLiveIdentity {
    publication: IndexPublicationRecord,
    cache_rows: Vec<(String, String, Vec<u8>, i64)>,
}

fn parser_cache_live_identity(storage_path: &Path) -> ParserCacheLiveIdentity {
    let storage = Storage::open_read_only(storage_path).expect("open parser publication");
    let publication = storage
        .get_complete_index_publication()
        .expect("read parser core publication")
        .expect("complete parser core publication");
    let cache_rows = {
        let mut statement = storage
            .get_connection()
            .prepare(
                "SELECT file_path, cache_key, artifact_blob, updated_at_epoch_ms
                 FROM index_artifact_cache
                 ORDER BY file_path",
            )
            .expect("prepare parser cache identity");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query parser cache identity")
            .map(|row| row.expect("read parser cache identity"))
            .collect()
    };
    ParserCacheLiveIdentity {
        publication,
        cache_rows,
    }
}

fn structural_live_identity(storage_path: &Path) -> StructuralLiveIdentity {
    let storage = Storage::open_read_only(storage_path).expect("open structural publication");
    let publication = storage
        .get_complete_index_publication()
        .expect("read structural core publication")
        .expect("complete structural core publication");
    let manifest = storage
        .validate_structural_text_unit_publication(&publication)
        .expect("validate structural publication");
    let cache_rows = {
        let mut statement = storage
            .get_connection()
            .prepare(
                "SELECT file_path, file_id, cache_key, source_content_hash,
                        descriptor_version, producer, artifact_digest
                 FROM structural_text_artifact_cache
                 ORDER BY file_path",
            )
            .expect("prepare structural cache identity");
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .expect("query structural cache identity")
            .map(|row| row.expect("read structural cache identity"))
            .collect()
    };
    StructuralLiveIdentity {
        publication,
        manifest,
        cache_rows,
    }
}

fn assert_no_staged_publication_artifacts(storage_path: &Path) {
    let parent = storage_path.parent().expect("storage parent");
    let staged = fs::read_dir(parent)
        .expect("list storage parent")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains(".staged."))
        .collect::<Vec<_>>();
    assert!(staged.is_empty(), "staged publication debris: {staged:?}");
}

fn storage_has_symbol(storage: &Storage, name: &str) -> bool {
    storage
        .get_nodes()
        .expect("read publication nodes")
        .iter()
        .any(|node| node.serialized_name == name)
}

fn copy_publication_fixture_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create publication fixture directory");
    for entry in fs::read_dir(source).expect("list publication fixture directory") {
        let entry = entry.expect("read publication fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            copy_publication_fixture_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy publication fixture file");
        }
    }
}

fn assert_publication_transition_fault_is_atomic(
    workspace_root: &Path,
    storage_path: &Path,
    baseline: &IndexPublicationRecord,
    baseline_search_generations: &[String],
    mode: IndexMode,
    boundary: PublicationTestBoundary,
    action: PublicationTestAction,
) {
    let source_path = workspace_root.join("lib.rs");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace_root.to_path_buf(),
            storage_path.to_path_buf(),
        )
        .expect("open baseline project");

    fs::write(&source_path, "pub fn new_generation() -> i32 { 2 }\n")
        .expect("write replacement source");
    let cancel_token = CancellationToken::new();
    arm_publication_test_fault(boundary, action);
    let result = controller.run_indexing_blocking_with_cancel(mode, &cancel_token);
    PUBLICATION_TEST_FAULT.with(|fault| {
        assert!(
            fault.borrow().is_none(),
            "publication fault was not reached: {mode:?} {boundary:?} {action:?}"
        );
    });
    let after_point_of_no_return = boundary == PublicationTestBoundary::RuntimeCache;
    if after_point_of_no_return {
        result.expect("post-commit faults must complete the committed publication");
        assert_eq!(
            cancel_token.is_cancelled(),
            action == PublicationTestAction::Cancel
        );
    } else {
        let error = result.expect_err("injected publication transition must fail visibly");
        match action {
            PublicationTestAction::Fail => {
                assert_eq!(error.code, "internal");
                assert!(error.message.contains(&format!("{boundary:?}")));
                assert!(!cancel_token.is_cancelled());
            }
            PublicationTestAction::Cancel => {
                assert_eq!(error.code, "cancelled");
                assert!(cancel_token.is_cancelled());
            }
        }
    }
    let state = controller.state.lock();
    assert!(!state.is_indexing);
    if after_point_of_no_return {
        assert_eq!(
            state
                .search_publication
                .as_ref()
                .expect("late cancellation published runtime search state")
                .generation,
            baseline.generation + 1
        );
    } else {
        assert_eq!(
            state
                .search_publication
                .as_ref()
                .expect("failed publication must restore baseline runtime search state")
                .generation,
            baseline.generation
        );
    }
    drop(state);
    assert_no_staged_publication_artifacts(storage_path);

    let replacement_reached_live = after_point_of_no_return;
    let publication_completed = replacement_reached_live;
    {
        let storage = Storage::open(storage_path).expect("open post-fault storage");
        let raw = storage
            .get_index_publication()
            .expect("read post-fault publication")
            .expect("post-fault publication identity");
        if replacement_reached_live {
            assert_eq!(raw.generation, baseline.generation + 1);
            assert!(storage_has_symbol(&storage, "new_generation"));
            assert!(!storage_has_symbol(&storage, "old_generation"));
            if publication_completed {
                assert!(
                    !storage
                        .has_incomplete_incremental_run()
                        .expect("read completed publication fence")
                );
                assert_eq!(
                    storage
                        .get_complete_index_publication()
                        .expect("read completed publication"),
                    Some(raw.clone())
                );
            } else {
                assert!(
                    storage
                        .has_incomplete_incremental_run()
                        .expect("read post-fault fence")
                );
                assert_eq!(
                    storage
                        .get_complete_index_publication()
                        .expect("read fenced complete publication"),
                    None,
                    "an interrupted incremental replacement must not be served"
                );
            }
        } else {
            assert_eq!(&raw, baseline);
            assert!(storage_has_symbol(&storage, "old_generation"));
            assert!(!storage_has_symbol(&storage, "new_generation"));
            assert_eq!(
                storage
                    .get_complete_index_publication()
                    .expect("read preserved complete publication"),
                Some(baseline.clone())
            );
            assert_eq!(
                persisted_search_generation_names(storage_path),
                baseline_search_generations,
                "an unpublished generation must be cleaned"
            );
        }
    }

    let restarted = AppController::new();
    let summary = restarted
        .open_project_summary_with_storage_path(
            workspace_root.to_path_buf(),
            storage_path.to_path_buf(),
        )
        .expect("restart against post-fault storage");
    if publication_completed {
        assert_eq!(
            summary
                .publication
                .expect("restart complete publication")
                .generation,
            baseline.generation + 1
        );
    } else if replacement_reached_live {
        assert!(
            summary.publication.is_none(),
            "fenced generation was served"
        );
        let dry_run_error = restarted
            .dry_run_index(IndexMode::Incremental)
            .expect_err("dry-run must reject implicit recovery escalation");
        assert_eq!(dry_run_error.code, FULL_REFRESH_REQUIRED_ERROR_CODE);
        assert_eq!(
            dry_run_error
                .details
                .as_ref()
                .and_then(|details| details.cause_code.as_deref()),
            Some("incomplete_incremental_publication")
        );
        let execution_error = restarted
            .run_indexing_blocking(IndexMode::Incremental)
            .expect_err("execution must reject implicit recovery escalation");
        assert_eq!(execution_error.code, FULL_REFRESH_REQUIRED_ERROR_CODE);
        restarted
            .run_indexing_blocking(IndexMode::Full)
            .expect("explicit full publication recovery");
        let storage = Storage::open(storage_path).expect("open recovered storage");
        let recovered = storage
            .get_complete_index_publication()
            .expect("read recovered publication")
            .expect("complete recovered publication");
        assert!(recovered.generation > baseline.generation);
        assert!(storage_has_symbol(&storage, "new_generation"));
        assert!(!storage_has_symbol(&storage, "old_generation"));
    } else {
        assert_eq!(
            summary
                .publication
                .expect("restart preserved publication")
                .generation,
            baseline.generation
        );
        let storage = Storage::open(storage_path).expect("open restarted baseline");
        assert!(storage_has_symbol(&storage, "old_generation"));
        assert!(!storage_has_symbol(&storage, "new_generation"));
    }
    assert_no_staged_publication_artifacts(storage_path);
}

fn assert_publication_transition_matrix(mode: IndexMode) {
    let workspace = tempdir().expect("workspace dir");
    let source_path = workspace.path().join("lib.rs");
    fs::write(&source_path, "pub fn old_generation() -> i32 { 1 }\n")
        .expect("write baseline source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let (baseline, baseline_search_generations) = {
        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open matrix baseline");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("publish matrix baseline");
        let storage = Storage::open(&storage_path).expect("open matrix baseline storage");
        assert!(storage_has_symbol(&storage, "old_generation"));
        (
            storage
                .get_complete_index_publication()
                .expect("read matrix baseline publication")
                .expect("complete matrix baseline publication"),
            persisted_search_generation_names(&storage_path),
        )
    };
    let backup = tempdir().expect("baseline backup");
    let backup_cache = backup.path().join("cache");
    copy_publication_fixture_directory(
        storage_path.parent().expect("matrix cache directory"),
        &backup_cache,
    );

    for boundary in PUBLICATION_TRANSITION_BOUNDARIES {
        if boundary == PublicationTestBoundary::MarkerCompletion && mode != IndexMode::Incremental {
            continue;
        }
        if matches!(
            boundary,
            PublicationTestBoundary::SemanticNodePage
                | PublicationTestBoundary::SemanticEndpointRead
        ) && mode != IndexMode::Full
        {
            continue;
        }
        for action in [PublicationTestAction::Fail, PublicationTestAction::Cancel] {
            eprintln!("publication matrix: {mode:?} {boundary:?} {action:?}");
            fs::remove_dir_all(storage_path.parent().expect("matrix cache directory"))
                .expect("reset matrix cache");
            copy_publication_fixture_directory(
                &backup_cache,
                storage_path.parent().expect("matrix cache directory"),
            );
            fs::write(&source_path, "pub fn old_generation() -> i32 { 1 }\n")
                .expect("reset matrix source");
            assert_publication_transition_fault_is_atomic(
                workspace.path(),
                &storage_path,
                &baseline,
                &baseline_search_generations,
                mode,
                boundary,
                action,
            );
        }
    }
}

#[test]
fn runtime_service_shared_cancellation_stops_full_refresh_before_core_publication() {
    let workspace = tempdir().expect("workspace dir");
    let source_path = workspace.path().join("lib.rs");
    fs::write(&source_path, "pub fn old_generation() -> i32 { 1 }\n")
        .expect("write baseline source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open shared-cancellation baseline");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("publish shared-cancellation baseline");
    let baseline = Storage::open(&storage_path)
        .expect("open shared-cancellation baseline")
        .get_complete_index_publication()
        .expect("read shared-cancellation baseline")
        .expect("complete shared-cancellation baseline");

    fs::write(&source_path, "pub fn new_generation() -> i32 { 2 }\n")
        .expect("write replacement source");
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    arm_publication_test_fault(
        PublicationTestBoundary::SearchBuild,
        PublicationTestAction::Cancel,
    );
    let error = crate::services::IndexService::new(controller)
        .run_indexing_blocking_with_cancel_flag(IndexMode::Full, Arc::clone(&cancelled))
        .expect_err("shared cancellation must stop the full refresh before publication");

    assert_eq!(error.code, "cancelled");
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    let storage = Storage::open(&storage_path).expect("open cancelled publication");
    assert_eq!(
        storage
            .get_complete_index_publication()
            .expect("read cancelled publication"),
        Some(baseline),
        "shared cancellation advanced the core publication"
    );
    assert!(storage_has_symbol(&storage, "old_generation"));
    assert!(!storage_has_symbol(&storage, "new_generation"));
    assert_no_staged_publication_artifacts(&storage_path);
}

fn assert_symbol_index_failure_preserves_previous_complete_publication(
    fault: search::engine::SymbolIndexTestFault,
    expected_error: &str,
) {
    let workspace = tempdir().expect("workspace dir");
    let source_path = workspace.path().join("lib.rs");
    fs::write(&source_path, "pub fn old_generation() -> i32 { 1 }\n")
        .expect("write baseline source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open add-failure baseline");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("publish add-failure baseline");
    let baseline = Storage::open(&storage_path)
        .expect("open add-failure baseline")
        .get_complete_index_publication()
        .expect("read add-failure baseline")
        .expect("complete add-failure baseline");

    fs::write(
        &source_path,
        "pub fn new_generation() -> i32 { 2 }\npub fn another_symbol() {}\n",
    )
    .expect("write replacement source");
    search::engine::arm_symbol_index_test_fault(fault);
    let error = controller
        .run_indexing_blocking(IndexMode::Full)
        .expect_err("symbol index failure must reject the candidate");

    assert!(error.message.contains(expected_error));
    let storage = Storage::open(&storage_path).expect("open preserved publication");
    assert_eq!(
        storage
            .get_complete_index_publication()
            .expect("read preserved publication"),
        Some(baseline)
    );
    assert!(storage_has_symbol(&storage, "old_generation"));
    assert!(!storage_has_symbol(&storage, "new_generation"));
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn symbol_index_add_failure_preserves_previous_complete_publication() {
    assert_symbol_index_failure_preserves_previous_complete_publication(
        search::engine::SymbolIndexTestFault::AddDocumentAfterOne,
        "add-document failure",
    );
}

#[test]
fn symbol_index_commit_failure_preserves_previous_complete_publication() {
    assert_symbol_index_failure_preserves_previous_complete_publication(
        search::engine::SymbolIndexTestFault::Commit,
        "commit failure",
    );
}

#[test]
fn cancelled_full_refresh_preserves_previous_verified_exclusion_manifest() {
    let workspace = tempdir().expect("workspace dir");
    let ordinary = workspace.path().join("lib.rs");
    let oversized = workspace.path().join("generated.rs");
    fs::write(&ordinary, "pub fn stable() -> i32 { 1 }\n").expect("write ordinary source");
    fs::write(&oversized, "pub fn generated() {}\n").expect("write generated source");
    make_source_exceed_default_index_byte_cap(&oversized, "baseline exclusion");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open exclusion cancellation baseline");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish exclusion cancellation baseline");
    let baseline_storage = Storage::open(&storage_path).expect("baseline storage");
    let baseline_publication = baseline_storage
        .get_complete_index_publication()
        .expect("baseline publication")
        .expect("complete baseline publication");
    let baseline_manifest = baseline_storage
        .get_source_policy_exclusion_manifest()
        .expect("baseline exclusion manifest")
        .expect("complete baseline exclusion manifest");
    let baseline_exclusions = baseline_storage
        .get_source_policy_exclusions()
        .expect("baseline exclusions");
    drop(baseline_storage);

    fs::write(
        &oversized,
        format!("{}\n// changed\n", fs::read_to_string(&oversized).unwrap()),
    )
    .expect("change excluded source");
    let cancel_token = CancellationToken::new();
    arm_publication_test_fault(
        PublicationTestBoundary::SearchBuild,
        PublicationTestAction::Cancel,
    );
    let error = controller
        .run_indexing_blocking_without_runtime_refresh_with_cancel(IndexMode::Full, &cancel_token)
        .expect_err("cancelled exclusion refresh must fail visibly");
    assert_eq!(error.code, "cancelled");

    let storage = Storage::open(&storage_path).expect("cancelled exclusion storage");
    assert_eq!(
        storage
            .get_complete_index_publication()
            .expect("preserved complete publication"),
        Some(baseline_publication)
    );
    assert_eq!(
        storage
            .get_source_policy_exclusion_manifest()
            .expect("preserved exclusion manifest"),
        Some(baseline_manifest)
    );
    assert_eq!(
        storage
            .get_source_policy_exclusions()
            .expect("preserved exclusions"),
        baseline_exclusions
    );
    assert_no_staged_publication_artifacts(&storage_path);
}

#[test]
fn full_recovery_marker_completion_fault_preserves_fenced_live_generation() {
    let workspace = tempdir().expect("workspace dir");
    let source_path = workspace.path().join("lib.rs");
    fs::write(&source_path, "pub fn old_generation() -> i32 { 1 }\n")
        .expect("write baseline source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let baseline = {
        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open recovery baseline");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("publish recovery baseline");
        Storage::open(&storage_path)
            .expect("open recovery baseline storage")
            .get_complete_index_publication()
            .expect("read recovery baseline")
            .expect("complete recovery baseline")
    };
    let backup = tempdir().expect("baseline backup");
    let backup_cache = backup.path().join("cache");
    copy_publication_fixture_directory(
        storage_path.parent().expect("recovery cache directory"),
        &backup_cache,
    );

    for action in [PublicationTestAction::Fail, PublicationTestAction::Cancel] {
        fs::remove_dir_all(storage_path.parent().expect("recovery cache directory"))
            .expect("reset recovery cache");
        copy_publication_fixture_directory(
            &backup_cache,
            storage_path.parent().expect("recovery cache directory"),
        );
        Storage::open(&storage_path)
            .expect("open interrupted live storage")
            .begin_incremental_run()
            .expect("fence interrupted live storage");
        fs::write(&source_path, "pub fn new_generation() -> i32 { 2 }\n")
            .expect("write recovery source");
        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open fenced recovery project");
        let cancel_token = CancellationToken::new();
        arm_publication_test_fault(PublicationTestBoundary::MarkerCompletion, action);
        let error = controller
            .run_indexing_blocking_with_cancel(IndexMode::Full, &cancel_token)
            .expect_err("pre-publication marker fault must fail");
        match action {
            PublicationTestAction::Fail => assert_eq!(error.code, "internal"),
            PublicationTestAction::Cancel => assert_eq!(error.code, "cancelled"),
        }
        let storage = Storage::open(&storage_path).expect("open preserved fenced live storage");
        assert_eq!(
            storage
                .get_index_publication()
                .expect("read preserved raw publication"),
            Some(baseline.clone())
        );
        assert!(
            storage
                .has_incomplete_incremental_run()
                .expect("read preserved incomplete marker")
        );
        assert_eq!(
            storage
                .get_complete_index_publication()
                .expect("read preserved complete publication"),
            None
        );
        assert!(storage_has_symbol(&storage, "old_generation"));
        assert!(!storage_has_symbol(&storage, "new_generation"));
        drop(storage);
        assert_no_staged_publication_artifacts(&storage_path);

        let restarted = AppController::new();
        restarted
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("restart fenced recovery project");
        restarted
            .run_indexing_blocking(IndexMode::Full)
            .expect("complete fenced recovery");
        let storage = Storage::open(&storage_path).expect("open completed recovery");
        assert_eq!(
            storage
                .get_complete_index_publication()
                .expect("read completed recovery")
                .expect("complete recovered publication")
                .generation,
            baseline.generation + 1
        );
        assert!(storage_has_symbol(&storage, "new_generation"));
    }
}

#[test]
fn full_publication_transitions_fail_or_cancel_atomically() {
    assert_publication_transition_matrix(IndexMode::Full);
}

#[test]
fn incremental_publication_transitions_fail_or_cancel_atomically() {
    assert_publication_transition_matrix(IndexMode::Incremental);
}

#[test]
fn index_writer_lock_reports_cache_busy_and_releases_after_drop() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");

    let guard = IndexWriterGuard::try_acquire(&storage_path).expect("first writer lock");
    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("second writer must be excluded");
    assert_eq!(error.code, "cache_busy");
    assert!(!controller.state.lock().is_indexing);

    drop(guard);
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("writer lock should be reusable after drop");
}

#[test]
fn first_incremental_requires_full_before_cancellation_or_storage_creation() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let (events_tx, _events_rx) = unbounded();
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let error = match index_incremental(
        workspace.path(),
        &storage_path,
        &events_tx,
        Some(&cancel_token),
    ) {
        Err(error) => error,
        Ok(_) => panic!("first incremental must fail visibly"),
    };

    assert_eq!(error.code, FULL_REFRESH_REQUIRED_ERROR_CODE);
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.cause_code.as_deref()),
        Some("complete_core_publication_missing")
    );
    assert!(
        !storage_path.exists(),
        "a rejected first incremental must not manufacture a live generation"
    );
    let cache_dir = storage_path.parent().expect("cache parent");
    if cache_dir.exists() {
        let staged_artifacts = fs::read_dir(cache_dir)
            .expect("list cache dir")
            .collect::<Result<Vec<_>, _>>()
            .expect("read cache entries");
        assert!(
            staged_artifacts.is_empty(),
            "rejected first incremental left staged debris: {staged_artifacts:?}"
        );
    }
}

#[test]
fn cancelled_incremental_preserves_live_generation_and_retries_incrementally() {
    let workspace = tempdir().expect("workspace dir");
    for index in 0..64 {
        fs::write(
            workspace.path().join(format!("module_{index}.rs")),
            format!(
                "pub fn caller_{index}() {{ callee_{index}(); }}\npub fn callee_{index}() {{}}\n"
            ),
        )
        .expect("write source");
    }
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");

    let (baseline_stats, baseline_snapshots, baseline_publication, baseline_search_generations) = {
        let storage = Storage::open(&storage_path).expect("open baseline storage");
        (
            storage.get_stats().expect("baseline stats"),
            storage
                .snapshots()
                .get_metadata()
                .expect("baseline snapshot metadata"),
            storage
                .get_index_publication()
                .expect("baseline publication read")
                .expect("baseline publication identity"),
            persisted_search_generation_names(&storage_path),
        )
    };
    fs::write(
        workspace.path().join("module_0.rs"),
        "pub fn replacement_value() -> i32 { 42 }\n",
    )
    .expect("change source before incremental");

    let events = controller.events();
    let cancel_token = CancellationToken::new();
    let cancel_from_progress = cancel_token.clone();
    let canceller = std::thread::spawn(move || {
        while let Ok(event) = events.recv_timeout(Duration::from_secs(10)) {
            if let AppEventPayload::IndexingProgress { current, total } = event
                && current == total
            {
                cancel_from_progress.cancel();
                return;
            }
        }
        panic!("incremental progress did not reach the cancellation boundary");
    });

    let error = controller
        .run_indexing_blocking_without_runtime_refresh_with_cancel(
            IndexMode::Incremental,
            &cancel_token,
        )
        .expect_err("cancelled incremental must fail visibly");
    canceller.join().expect("progress canceller");
    assert_eq!(error.code, "cancelled");
    let storage = Storage::open(&storage_path).expect("open cancelled storage");
    assert!(
        !storage
            .has_incomplete_incremental_run()
            .expect("cancelled live marker")
    );
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("cancelled live schema"),
        codestory_store::CURRENT_SCHEMA_VERSION
    );
    let cancelled_stats = storage.get_stats().expect("cancelled live stats");
    assert_eq!(cancelled_stats.node_count, baseline_stats.node_count);
    assert_eq!(cancelled_stats.edge_count, baseline_stats.edge_count);
    assert_eq!(cancelled_stats.file_count, baseline_stats.file_count);
    assert_eq!(
        storage
            .snapshots()
            .get_metadata()
            .expect("cancelled snapshot metadata"),
        baseline_snapshots
    );
    assert_eq!(
        storage
            .get_index_publication()
            .expect("cancelled live publication"),
        Some(baseline_publication)
    );
    assert_eq!(
        persisted_search_generation_names(&storage_path),
        baseline_search_generations,
        "cancelled incremental must not create search state for an unpublished generation"
    );
    assert!(
        storage
            .get_nodes()
            .expect("cancelled live nodes")
            .iter()
            .all(|node| node.serialized_name != "replacement_value")
    );
    drop(storage);

    let summary = controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("reopen cancelled project");
    assert_eq!(
        summary.freshness.expect("cancelled freshness").status,
        IndexFreshnessStatusDto::Stale
    );
    let dry_run = controller
        .dry_run_index(IndexMode::Incremental)
        .expect("dry-run direct retry");
    assert_eq!(dry_run.refresh, IndexMode::Incremental);
    let timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
        .expect("retry cancelled incremental");
    assert!(timings.publish_ms.is_some());
    let storage = Storage::open(&storage_path).expect("open retried storage");
    assert!(
        !storage
            .has_incomplete_incremental_run()
            .expect("marker after retry")
    );
    assert!(
        storage
            .get_nodes()
            .expect("retried nodes")
            .iter()
            .any(|node| node.serialized_name == "replacement_value")
    );
}

#[test]
fn cancelled_blocking_index_is_user_visible_and_clears_indexing_state() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");

    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let error = controller
        .run_indexing_blocking_without_runtime_refresh_with_cancel(IndexMode::Full, &cancel_token)
        .expect_err("cancelled indexing should be visible");

    assert_eq!(error.code, "cancelled");
    assert!(error.message.contains("cancelled"));
    assert!(!controller.state.lock().is_indexing);
}

#[test]
fn search_rejects_reads_while_indexing_is_active() {
    let controller = AppController::new();
    {
        let mut state = controller.state.lock();
        state.is_indexing = true;
    }

    let error = controller
        .search_results(SearchRequest {
            query: "check_winner".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("search should be blocked while indexing");

    assert_eq!(error.code, "invalid_argument");
    assert!(error.message.contains("indexing is in progress"));
}

#[test]
fn search_after_summary_open_stays_sidecar_primary_without_runtime_refresh() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let error = controller
        .search(SearchRequest {
            query: "check_winner".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("search should require full sidecars after summary open");

    assert_mandatory_retrieval_unavailable(&error);
    let state = controller.state.lock();
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn full_refresh_publishes_both_grounding_snapshot_tiers() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let storage = Storage::open(&storage_path).expect("reopen storage");
    assert!(
        storage
            .snapshots()
            .has_ready_summary()
            .expect("summary snapshot readiness"),
        "full refresh should publish ready grounding summary snapshots"
    );
    assert!(
        storage
            .snapshots()
            .has_ready_detail()
            .expect("detail snapshot readiness"),
        "full refresh should publish ready grounding detail snapshots"
    );
}

#[test]
fn normalized_hybrid_weights_clamps_and_normalizes_values() {
    let fallback = HybridSearchConfig::default();
    let (lexical, semantic, graph) = normalized_hybrid_weights(
        Some(AgentHybridWeightsDto {
            lexical: Some(2.0),
            semantic: Some(-1.0),
            graph: Some(0.5),
        }),
        &fallback,
    );

    assert!((lexical - 0.666_666_7).abs() < 1e-4);
    assert!((semantic - 0.0).abs() < 1e-6);
    assert!((graph - 0.333_333_34).abs() < 1e-4);
}

#[test]
fn normalized_hybrid_weights_falls_back_when_invalid_sum() {
    let fallback = HybridSearchConfig::default();
    let (lexical, semantic, graph) = normalized_hybrid_weights(
        Some(AgentHybridWeightsDto {
            lexical: Some(0.0),
            semantic: Some(0.0),
            graph: Some(0.0),
        }),
        &fallback,
    );

    assert!((lexical - fallback.lexical_weight).abs() < 1e-6);
    assert!((semantic - fallback.semantic_weight).abs() < 1e-6);
    assert!((graph - fallback.graph_weight).abs() < 1e-6);
}

#[test]
fn hybrid_search_defaults_to_accuracy_first_semantic_profile() {
    let config = HybridSearchConfig::default();

    assert_eq!(config.max_results, 20);
    assert_eq!(config.lexical_weight, 0.0);
    assert_eq!(config.semantic_weight, 1.0);
    assert_eq!(config.graph_weight, 0.0);
    assert_eq!(config.lexical_limit, 0);
    assert_eq!(config.semantic_limit, 20);
}

#[test]
fn apply_hybrid_limits_overrides_and_caps_values() {
    let mut config = HybridSearchConfig::default();
    apply_hybrid_limits(
        Some(codestory_contracts::api::SearchHybridLimitsDto {
            lexical: Some(0),
            semantic: Some(5_000),
        }),
        &mut config,
    );

    assert_eq!(config.lexical_limit, 0);
    assert_eq!(config.semantic_limit, 1_000);
}

#[test]
fn progress_forwarder_relays_progress_and_status_events() {
    let (event_tx, event_rx) = unbounded::<Event>();
    let (app_tx, app_rx) = unbounded::<AppEventPayload>();
    let handle = spawn_progress_forwarder(event_rx, app_tx);

    event_tx
        .send(Event::IndexingProgress {
            current: 3,
            total: 5,
        })
        .expect("send progress event");
    event_tx
        .send(Event::StatusUpdate {
            message: "ignore me".to_string(),
        })
        .expect("send status event");
    drop(event_tx);

    let forwarded = app_rx.recv().expect("receive forwarded event");
    assert!(matches!(
        forwarded,
        AppEventPayload::IndexingProgress {
            current: 3,
            total: 5
        }
    ));
    let status = app_rx.recv().expect("receive status update");
    assert!(matches!(
        status,
        AppEventPayload::StatusUpdate { message } if message == "ignore me"
    ));
    assert!(
        app_rx.try_recv().is_err(),
        "unexpected extra forwarded events"
    );
    handle.join().expect("join forwarder");
}

#[test]
fn write_file_text_writes_inside_project_root() {
    let temp = tempdir().expect("create temp dir");
    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let result = controller
        .write_file_text(WriteFileTextRequest {
            path: "notes.txt".to_string(),
            text: "hello world".to_string(),
        })
        .expect("write text file");

    assert_eq!(result.bytes_written, 11);
    let saved = std::fs::read_to_string(temp.path().join("notes.txt")).expect("read file");
    assert_eq!(saved, "hello world");
}

#[test]
fn write_file_text_rejects_paths_outside_project_root() {
    let temp = tempdir().expect("create temp dir");
    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let err = controller
        .write_file_text(WriteFileTextRequest {
            path: "../escape.txt".to_string(),
            text: "nope".to_string(),
        })
        .expect_err("write should fail");

    assert_eq!(err.code, "invalid_argument");
}

#[test]
fn list_root_symbols_deduplicates_repeated_entries() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::MODULE,
                    serialized_name: "\"react\"".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(102),
                    kind: NodeKind::MODULE,
                    serialized_name: "\"react\"".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(103),
                    kind: NodeKind::MODULE,
                    serialized_name: "\"./app/types\"".to_string(),
                    ..Default::default()
                },
            ])
            .expect("insert root nodes");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let roots = controller
        .list_root_symbols(ListRootSymbolsRequest { limit: None })
        .expect("load roots");
    let react_count = roots
        .iter()
        .filter(|symbol| symbol.label == "\"react\"")
        .count();

    assert_eq!(react_count, 1);
    assert!(roots.iter().any(|symbol| symbol.label == "\"./app/types\""));
}

#[test]
fn graph_neighborhood_member_includes_owner_inheritance_edges() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::INTERFACE,
                    serialized_name: "EventListener".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "EventListener::handle_event".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::CLASS,
                    serialized_name: "UiListener".to_string(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                Edge {
                    id: EdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: EdgeKind::MEMBER,
                    ..Default::default()
                },
                Edge {
                    id: EdgeId(12),
                    source: CoreNodeId(3),
                    target: CoreNodeId(1),
                    kind: EdgeKind::INHERITANCE,
                    ..Default::default()
                },
            ])
            .expect("insert edges");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let graph = controller
        .graph_neighborhood(GraphRequest {
            center_id: codestory_contracts::api::NodeId("2".to_string()),
            max_edges: None,
        })
        .expect("load graph neighborhood");

    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.kind == codestory_contracts::api::EdgeKind::INHERITANCE),
        "Expected INHERITANCE edge from owner trait context"
    );
    assert!(
        graph.canonical_layout.is_some(),
        "Expected canonical_layout on neighborhood response"
    );
}

#[test]
fn graph_trail_includes_canonical_layout() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::CLASS,
                    serialized_name: "Runner".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::METHOD,
                    serialized_name: "Runner::run".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::METHOD,
                    serialized_name: "Worker::execute".to_string(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                Edge {
                    id: EdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: EdgeKind::MEMBER,
                    ..Default::default()
                },
                Edge {
                    id: EdgeId(12),
                    source: CoreNodeId(2),
                    target: CoreNodeId(3),
                    kind: EdgeKind::CALL,
                    ..Default::default()
                },
            ])
            .expect("insert edges");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let graph = controller
        .graph_trail(TrailConfigDto {
            root_id: codestory_contracts::api::NodeId("2".to_string()),
            mode: codestory_contracts::api::TrailMode::Neighborhood,
            target_id: None,
            depth: 2,
            direction: codestory_contracts::api::TrailDirection::Both,
            caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
            edge_filter: vec![],
            show_utility_calls: false,
            hide_speculative: false,
            story: false,
            node_filter: vec![],
            max_nodes: 128,
            layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
        })
        .expect("load graph trail");

    assert!(
        graph.canonical_layout.is_some(),
        "Expected canonical_layout on trail response"
    );
}

#[test]
fn graph_direct_references_returns_filtered_direct_incoming_edges() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::FILE,
                    serialized_name: "src/lib.rs".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::FILE,
                    serialized_name: "tests/lib_test.rs".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "target".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "prod_caller".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "test_caller".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(4),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "uncertain_caller".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                Edge {
                    id: EdgeId(21),
                    source: CoreNodeId(2),
                    target: CoreNodeId(1),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(CoreNodeId(10)),
                    certainty: Some(ResolutionCertainty::Certain),
                    confidence: Some(0.95),
                    ..Default::default()
                },
                Edge {
                    id: EdgeId(22),
                    source: CoreNodeId(3),
                    target: CoreNodeId(1),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(CoreNodeId(11)),
                    certainty: Some(ResolutionCertainty::Certain),
                    confidence: Some(0.95),
                    ..Default::default()
                },
                Edge {
                    id: EdgeId(23),
                    source: CoreNodeId(4),
                    target: CoreNodeId(1),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(CoreNodeId(10)),
                    certainty: Some(ResolutionCertainty::Uncertain),
                    confidence: Some(0.4),
                    ..Default::default()
                },
            ])
            .expect("insert edges");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let graph = controller
        .graph_direct_references(TrailConfigDto {
            root_id: codestory_contracts::api::NodeId("1".to_string()),
            mode: codestory_contracts::api::TrailMode::AllReferencing,
            target_id: None,
            depth: 0,
            direction: codestory_contracts::api::TrailDirection::Incoming,
            caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
            edge_filter: vec![],
            show_utility_calls: false,
            hide_speculative: true,
            story: false,
            node_filter: vec![],
            max_nodes: 10,
            layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
        })
        .expect("load direct references");

    let edge_sources = graph
        .edges
        .iter()
        .map(|edge| edge.source.0.as_str())
        .collect::<Vec<_>>();
    assert_eq!(edge_sources, vec!["2"]);
    let node_ids = graph
        .nodes
        .iter()
        .map(|node| node.id.0.as_str())
        .collect::<Vec<_>>();
    assert_eq!(node_ids, vec!["1", "2"]);
    assert!(graph.canonical_layout.is_none());
}

#[test]
fn high_fanout_graph_trail_reports_truncation_at_max_nodes() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        let mut nodes = vec![Node {
            id: CoreNodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "root".to_string(),
            ..Default::default()
        }];
        let mut edges = Vec::new();
        for idx in 2..80 {
            nodes.push(Node {
                id: CoreNodeId(idx),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("child_{idx}"),
                ..Default::default()
            });
            edges.push(Edge {
                id: EdgeId(idx + 100),
                source: CoreNodeId(1),
                target: CoreNodeId(idx),
                kind: EdgeKind::CALL,
                ..Default::default()
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage.insert_edges_batch(&edges).expect("insert edges");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let graph = controller
        .graph_trail(TrailConfigDto {
            root_id: codestory_contracts::api::NodeId("1".to_string()),
            mode: codestory_contracts::api::TrailMode::Neighborhood,
            target_id: None,
            depth: 1,
            direction: codestory_contracts::api::TrailDirection::Outgoing,
            caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
            edge_filter: vec![],
            show_utility_calls: true,
            hide_speculative: false,
            story: false,
            node_filter: vec![],
            max_nodes: 10,
            layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
        })
        .expect("load high fanout trail");

    assert!(graph.truncated, "expected trail truncation: {graph:?}");
    assert!(graph.nodes.len() <= 10);
}

#[test]
fn update_bookmark_category_returns_not_found_when_missing() {
    let temp = tempdir().expect("create temp dir");
    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let err = controller
        .update_bookmark_category(
            9_999,
            UpdateBookmarkCategoryRequest {
                name: "Renamed".to_string(),
            },
        )
        .expect_err("missing category should return not_found");

    assert_eq!(err.code, "not_found");
}
