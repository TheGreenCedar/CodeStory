//! Binds the recorded W6.8 bake-off outcome (#1664) to the contract that
//! produced it, and binds the harness's incumbent column to the shipped dense
//! scan.
//!
//! Two separate failures are guarded here. The first is an evidence file whose
//! verdict was written by hand rather than derived from its own measurements —
//! the recorded disposition is recomputed from the recorded gates and outcomes
//! and must match. The second is a harness whose "incumbent" is a convenient
//! reimplementation: the benchmark surface is exercised for two behaviours only
//! `codestory_retrieval`'s real scan has.

use std::collections::BTreeSet;
use std::path::PathBuf;

use codestory_bench::vector_backend_bakeoff::{
    BAKEOFF_SCHEMA, BakeoffGates, BakeoffResult, CandidateId, CorpusProvenance, Disposition,
    RETAIN_INCUMBENT_NON_CLAIM, evaluate_disposition,
};
use codestory_retrieval::benchmark_support::{
    BenchmarkVector, publish_vector_generation, read_published_vectors, scan_published_vectors,
    semantic_stage_budget_ms,
};

const BUDGET_PROBE_QUERY: &str = "how does the request dispatcher choose a transport adapter";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("bench crate lives two levels below the repository root")
        .to_path_buf()
}

fn recorded_path() -> PathBuf {
    repo_root().join("benchmarks/release-evidence/vector-backend-bakeoff/macos-arm64.json")
}

fn recorded() -> BakeoffResult {
    let path = recorded_path();
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn the_recorded_outcome_parses_against_the_shipped_schema() {
    let record = recorded();
    assert_eq!(record.schema, BAKEOFF_SCHEMA);
    assert!(!record.host.is_empty());
    assert!(!record.recorded_at.is_empty());
}

#[test]
fn every_declared_candidate_is_accounted_for_in_the_recorded_run() {
    // A candidate silently missing from the record is how a bake-off "passes"
    // by never asking the awkward question.
    let record = recorded();
    for candidate in CandidateId::ALL {
        assert!(
            record.outcome(candidate).is_some(),
            "{} is missing from the recorded bake-off",
            candidate.as_str()
        );
    }
    assert_eq!(record.outcomes.len(), CandidateId::ALL.len());
}

#[test]
fn the_recorded_verdict_is_derivable_from_the_recorded_measurements() {
    // The disposition in the file is not trusted: it is recomputed by the
    // shipped evaluator from the file's own gates and outcomes. An adoption
    // typed into the evidence, or a measurement edited after the verdict was
    // written, fails here.
    let record = recorded();
    assert_eq!(
        evaluate_disposition(&record.gates, &record.outcomes),
        record.disposition
    );
}

#[test]
fn the_recorded_run_ships_the_declared_gates_not_weakened_ones() {
    let record = recorded();
    assert_eq!(
        record.gates,
        BakeoffGates::declared(record.gates.semantic_stage_budget_ms)
    );
}

#[test]
fn the_recorded_stage_budget_still_matches_the_shipped_retrieval_planner() {
    // The timeout gate is only meaningful against the budget the product
    // actually gives the semantic stage. If that budget moves, this evidence
    // is stale and has to be re-run rather than quietly reinterpreted.
    let record = recorded();
    assert_eq!(
        Some(record.gates.semantic_stage_budget_ms),
        semantic_stage_budget_ms(BUDGET_PROBE_QUERY)
    );
}

#[test]
fn the_recorded_run_adopts_no_backend_and_states_the_non_claim() {
    let record = recorded();
    let Disposition::RetainIncumbent {
        non_claim,
        blocking_reasons,
    } = &record.disposition
    else {
        panic!(
            "the recorded bake-off adopted a backend: {:?}. Adopting one is an implementation \
             change that has to land with the backend, not with the measurement.",
            record.disposition
        );
    };
    assert_eq!(non_claim, RETAIN_INCUMBENT_NON_CLAIM);
    for candidate in CandidateId::ALL {
        if candidate.is_incumbent() {
            continue;
        }
        assert!(
            blocking_reasons
                .iter()
                .any(|reason| reason.starts_with(candidate.as_str())),
            "no recorded reason why {} did not qualify",
            candidate.as_str()
        );
    }
}

#[test]
fn a_non_representative_run_says_so_in_its_limitations() {
    let record = recorded();
    if record.corpus_provenance == CorpusProvenance::Representative {
        return;
    }
    assert!(
        record
            .limitations
            .iter()
            .any(|limitation| limitation.contains("no answer-quality signal")),
        "a synthetic run must state that it carries no quality signal: {:?}",
        record.limitations
    );
}

/// A corpus whose exact cosine ranking against `e0` is known by construction.
fn abstention_corpus() -> Vec<BenchmarkVector> {
    fn unit(values: [f32; 8]) -> Vec<f32> {
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        values.iter().map(|value| value / norm).collect()
    }
    vec![
        // cosine 1.000 against the query
        BenchmarkVector {
            node_id: "aligned".to_string(),
            vector: unit([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        },
        // cosine 0.707 — above the lane's half-of-best floor
        BenchmarkVector {
            node_id: "near".to_string(),
            vector: unit([1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        },
        // cosine 0.316 — below the floor
        BenchmarkVector {
            node_id: "weak".to_string(),
            vector: unit([1.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        },
        // cosine 0.000
        BenchmarkVector {
            node_id: "orthogonal".to_string(),
            vector: unit([0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        },
        // cosine -1.000
        BenchmarkVector {
            node_id: "opposed".to_string(),
            vector: unit([-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        },
    ]
}

#[test]
fn the_benchmark_scan_is_the_product_scan_including_its_abstention_rule() {
    // Five neighbours are published and a window of five is asked for, so a
    // scan that merely sorted would return all five. The shipped lane keeps
    // only neighbours at or above half its own best similarity, and that is
    // what the harness must be measuring.
    let root = tempfile::tempdir().expect("bake-off root");
    let corpus = abstention_corpus();
    let published = publish_vector_generation(
        root.path(),
        "abstention",
        "generation-1",
        "input-1",
        8,
        &corpus,
    )
    .expect("publish through the product path");
    assert_eq!(published.point_count(), 5);

    let query = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let hits = scan_published_vectors(&published, &query, 5, &|| false).expect("scan");
    let served = hits
        .iter()
        .map(|(node_id, _)| node_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(served, vec!["aligned".to_string(), "near".to_string()]);
}

#[test]
fn the_benchmark_scan_honours_cancellation_like_the_product_stage() {
    let root = tempfile::tempdir().expect("bake-off root");
    let published = publish_vector_generation(
        root.path(),
        "cancel",
        "generation-1",
        "input-1",
        8,
        &abstention_corpus(),
    )
    .expect("publish through the product path");
    let query = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let error = scan_published_vectors(&published, &query, 5, &|| true)
        .expect_err("a cancelled scan must fail rather than return a partial window");
    assert!(
        format!("{error:#}").contains("cancelled"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn candidates_read_back_exactly_the_bytes_the_incumbent_scans() {
    // If the read-back path diverged from the published table, a candidate
    // backend would be measured against a different corpus than the incumbent.
    let root = tempfile::tempdir().expect("bake-off root");
    let corpus = abstention_corpus();
    let published = publish_vector_generation(
        root.path(),
        "readback",
        "generation-1",
        "input-1",
        8,
        &corpus,
    )
    .expect("publish through the product path");
    let mut loaded = read_published_vectors(&published, 8).expect("read back");
    loaded.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    let mut expected = corpus;
    expected.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    assert_eq!(loaded, expected);
    assert!(published.database_bytes().expect("database bytes") > 0);
}

#[test]
fn the_declared_platform_set_is_the_shipped_package_set() {
    // The bake-off's platform requirement has to be the packages the release
    // actually claims, not a subset that happens to be convenient to run.
    let claims: serde_json::Value = serde_json::from_slice(
        &std::fs::read(repo_root().join("release-claims.json")).expect("read release claims"),
    )
    .expect("parse release claims");
    let shipped = claims["public_support"]["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .map(|package| {
            package["target"]
                .as_str()
                .expect("package target")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(BakeoffGates::declared(250).required_platforms, shipped);
}
