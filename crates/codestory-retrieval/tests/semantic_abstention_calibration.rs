#![cfg(feature = "semantic-calibration-support")]

use codestory_retrieval::semantic_calibration_support::{
    CALIBRATION_CORPUS_SCHEMA_VERSION, CALIBRATION_EDGE_CONTRACT_PATH, CALIBRATION_FEATURE,
    CALIBRATION_FIXTURE_PATH, CALIBRATION_FIXTURE_TRANSFORMATION,
    CALIBRATION_HOLDOUT_MANIFEST_PATH, CalibrationCandidate, CalibrationCaptureIdentity,
    CalibrationExpectedCall, CalibrationFixtureIdentity, CalibrationMetrics, CalibrationPolicy,
    CalibrationQuery, CalibrationSelection, CalibrationSelectionContract,
    SemanticCalibrationCorpus, development_queries, hex_bytes, load_attested_corpus,
    materialize_public_owner_fixture, query_vector_bytes, select_policy, sha256_bytes,
    validate_attested_repository_inputs, validate_holdout_disjointness,
};
use std::path::{Path, PathBuf};

#[test]
fn semantic_calibration_missing_attested_corpus_fails_closed() {
    let empty = tempfile::tempdir().expect("empty calibration directory");
    let error = load_attested_corpus(empty.path(), &repository_root())
        .expect_err("missing corpus must block selection");
    assert!(
        error
            .to_string()
            .contains("attested semantic calibration corpus is unavailable"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn semantic_calibration_selector_uses_the_exact_grid_constraints_and_tie_breaks() {
    let mut corpus = selector_contract_fixture();
    let selected = select_policy(&corpus).expect("selector contract has a valid pair");
    assert_eq!(
        selected.policy,
        CalibrationPolicy {
            absolute_floor_hundredths: 30,
            additive_margin_hundredths: 10,
        }
    );
    assert_eq!(selected.baseline.relevant_at_10, 2);
    assert_eq!(selected.metrics.relevant_at_10, 2);
    assert_eq!(selected.baseline.noisy_query_false_positives, 1);
    assert_eq!(selected.metrics.noisy_query_false_positives, 0);
    assert_eq!(selected.metrics.retained_candidates, 2);
    corpus.selection = selected.clone();
    assert_eq!(
        select_policy(&corpus).expect("deterministic replay"),
        selected
    );
}

#[test]
fn semantic_calibration_generator_inputs_are_source_backed_and_holdout_disjoint() {
    let root = repository_root();
    let fixture =
        std::fs::read_to_string(root.join(CALIBRATION_FIXTURE_PATH)).expect("read source fixture");
    let edge_contract = std::fs::read_to_string(root.join(CALIBRATION_EDGE_CONTRACT_PATH))
        .expect("read edge contract");
    let materialized =
        materialize_public_owner_fixture(&fixture).expect("materialize public owner anchors");
    let mut corpus = selector_contract_fixture();
    corpus.capture.source_commit = "f90a1cc063175bf0f4b19f870774dd0e3e3dba29".into();
    corpus.fixture.source_sha256 = sha256_bytes(fixture.as_bytes());
    corpus.fixture.edge_contract_sha256 = sha256_bytes(edge_contract.as_bytes());
    corpus.fixture.materialized_sha256 = sha256_bytes(materialized.as_bytes());
    corpus.queries = development_queries()
        .iter()
        .map(|spec| {
            let expected_call =
                spec.expected_call
                    .map(
                        |(caller, caller_owner, callee_owner, callee)| CalibrationExpectedCall {
                            caller: caller.into(),
                            caller_owner: caller_owner.into(),
                            callee_owner: callee_owner.into(),
                            callee: callee.into(),
                        },
                    );
            let relevant_names = expected_call
                .as_ref()
                .map(|edge| vec![edge.caller_owner.clone(), edge.callee_owner.clone()])
                .unwrap_or_default();
            CalibrationQuery {
                task_id: spec.task_id.into(),
                query: spec.query.into(),
                query_sha256: sha256_bytes(spec.query.as_bytes()),
                query_vector_sha256: sha256_bytes(&query_vector_bytes(&[1.0])),
                query_vector_f32_le_hex: hex_bytes(&query_vector_bytes(&[1.0])),
                expected_call,
                noise_nonce: spec.noise_nonce.map(str::to_string),
                candidates: relevant_names
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| candidate(&name, 0.4 - index as f32 * 0.1, index + 1))
                    .collect(),
            }
        })
        .collect();
    let holdout =
        std::fs::read(root.join(CALIBRATION_HOLDOUT_MANIFEST_PATH)).expect("read holdout manifest");
    corpus.fixture.disjointness_manifest_sha256 = sha256_bytes(&holdout);
    validate_attested_repository_inputs(&corpus, &root)
        .expect("attested inputs remain source-backed and disjoint");
    validate_holdout_disjointness(&corpus, &holdout)
        .expect("development task ids, source commit, and query hashes are disjoint");
    let holdout_json: serde_json::Value = serde_json::from_slice(&holdout).expect("parse holdout");
    let first_holdout = &holdout_json["tasks"][0];
    let mut overlapping = corpus.clone();
    overlapping.queries[0].task_id = first_holdout["id"]
        .as_str()
        .expect("holdout task id")
        .into();
    validate_holdout_disjointness(&overlapping, &holdout)
        .expect_err("holdout task id must block calibration");
    let mut overlapping = corpus.clone();
    overlapping.capture.source_commit = first_holdout["repo"]["ref"]
        .as_str()
        .expect("holdout repository ref")
        .into();
    validate_holdout_disjointness(&overlapping, &holdout)
        .expect_err("holdout repository ref must block calibration");
    let mut overlapping = corpus.clone();
    overlapping.queries[0].query_sha256 = sha256_bytes(
        first_holdout["prompt"]
            .as_str()
            .expect("holdout prompt")
            .as_bytes(),
    );
    validate_holdout_disjointness(&overlapping, &holdout)
        .expect_err("holdout prompt hash must block calibration");
    assert_eq!(
        corpus.fixture.transformation_id,
        CALIBRATION_FIXTURE_TRANSFORMATION
    );
}

#[test]
fn checked_in_semantic_calibration_replays_the_product_policy() {
    let corpus = load_attested_corpus(
        &repository_root()
            .join("crates/codestory-retrieval/testdata/semantic-abstention-calibration-v1"),
        &repository_root(),
    )
    .expect("checked-in semantic calibration evidence");
    assert_eq!(
        corpus.selection.policy,
        CalibrationPolicy {
            absolute_floor_hundredths: 30,
            additive_margin_hundredths: 10,
        }
    );
    assert_eq!(corpus.selection.metrics.relevant_at_10, 9);
    assert_eq!(corpus.selection.metrics.relevant_total, 9);
    assert_eq!(corpus.selection.metrics.noisy_query_false_positives, 0);
    assert!(
        corpus.selection.metrics.retained_candidates
            <= corpus
                .selection
                .baseline
                .retained_candidates
                .saturating_mul(5)
                / 4
    );
}

fn selector_contract_fixture() -> SemanticCalibrationCorpus {
    let vector_bytes = query_vector_bytes(&[1.0]);
    let answer_query = "selector contract answer";
    let noise_query = "selectorcontractabsent";
    SemanticCalibrationCorpus {
        schema_version: CALIBRATION_CORPUS_SCHEMA_VERSION,
        capture: CalibrationCaptureIdentity {
            source_commit: "1111111111111111111111111111111111111111".into(),
            capture_feature: CALIBRATION_FEATURE.into(),
            cli_sha256: "2".repeat(64),
            vector_generation_manifest_file: "vector-generation-manifest.json".into(),
            vector_generation_manifest_sha256: "3".repeat(64),
            vector_database_file: "vectors.sqlite3".into(),
            vector_database_sha256: "4".repeat(64),
            capture_command: "capture".into(),
        },
        fixture: CalibrationFixtureIdentity {
            source_path: CALIBRATION_FIXTURE_PATH.into(),
            source_sha256: "5".repeat(64),
            edge_contract_path: CALIBRATION_EDGE_CONTRACT_PATH.into(),
            edge_contract_sha256: "6".repeat(64),
            transformation_id: CALIBRATION_FIXTURE_TRANSFORMATION.into(),
            materialized_sha256: "7".repeat(64),
            disjointness_manifest_path: CALIBRATION_HOLDOUT_MANIFEST_PATH.into(),
            disjointness_manifest_sha256: "8".repeat(64),
        },
        selection_contract: CalibrationSelectionContract::exact_grid(),
        queries: vec![
            CalibrationQuery {
                task_id: "selector-answer".into(),
                query: answer_query.into(),
                query_sha256: sha256_bytes(answer_query.as_bytes()),
                query_vector_sha256: sha256_bytes(&vector_bytes),
                query_vector_f32_le_hex: hex_bytes(&vector_bytes),
                expected_call: Some(CalibrationExpectedCall {
                    caller: "run".into(),
                    caller_owner: "Workflow".into(),
                    callee_owner: "Notifier".into(),
                    callee: "notify_event".into(),
                }),
                noise_nonce: None,
                candidates: vec![candidate("Workflow", 0.4, 1), candidate("Notifier", 0.3, 2)],
            },
            CalibrationQuery {
                task_id: "selector-noise".into(),
                query: noise_query.into(),
                query_sha256: sha256_bytes(noise_query.as_bytes()),
                query_vector_sha256: sha256_bytes(&vector_bytes),
                query_vector_f32_le_hex: hex_bytes(&vector_bytes),
                expected_call: None,
                noise_nonce: Some(noise_query.into()),
                candidates: vec![candidate("Component", 0.08, 1)],
            },
        ],
        selection: CalibrationSelection {
            baseline: CalibrationMetrics::default(),
            policy: CalibrationPolicy {
                absolute_floor_hundredths: 0,
                additive_margin_hundredths: 0,
            },
            metrics: CalibrationMetrics::default(),
        },
    }
}

fn candidate(name: &str, score: f32, rank: usize) -> CalibrationCandidate {
    CalibrationCandidate {
        node_id: format!("node-{name}"),
        document_hash: format!("document-{name}"),
        display_name: name.into(),
        file_path: "workflow.rs".into(),
        raw_score_bits: score.to_bits(),
        rank,
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}
