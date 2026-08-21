#[path = "../src/bin/codestory_proof_availability/cli.rs"]
mod cli;
#[path = "../src/bin/codestory_proof_availability/contracts.rs"]
mod contracts;

use clap::Parser;
use contracts::{
    ActivationDecisionV1, CaseReportV1, CorpusV1, InventoryReportV1, OraclePathV1,
    QualificationSummaryV1, SchemaDocument, ThresholdsV1, TrailReportV1,
};
use serde_json::{Value, json};

fn source() -> Value {
    json!({
        "source_id": "rust-fixture",
        "repository": "https://example.invalid/rust-fixture.git",
        "commit": "a".repeat(40),
        "tree": "b".repeat(40),
        "source_hashes": [{
            "path": "src/lib.rs",
            "sha256": "c".repeat(64),
            "byte_length": 128
        }]
    })
}

fn source_range(start_byte: u64, end_byte: u64) -> Value {
    json!({
        "path": "src/lib.rs",
        "start_byte": start_byte,
        "end_byte": end_byte,
        "sha256": "c".repeat(64)
    })
}

fn oracle_path(path_id: &str, source_range: Value) -> Value {
    json!({
        "schema": "codestory.proof-availability-path/v1",
        "path_id": path_id,
        "source_id": "rust-fixture",
        "expected_step_count": 2,
        "steps": [
            {"step_id": format!("{path_id}-start"), "selector": "crate::start", "source_range": source_range.clone()},
            {"step_id": format!("{path_id}-target"), "selector": "crate::target", "source_range": source_range}
        ],
        "expected_outcome": "proven",
        "notes": "maximal oracle path fixture"
    })
}

fn corpus() -> Value {
    json!({
        "schema": "codestory.proof-availability-corpus/v1",
        "corpus_id": "availability-fixtures-v1",
        "sources": [source()],
        "path_count": 2,
        "paths": [
            oracle_path("rust-two-step", source_range(0, 32)),
            oracle_path("rust-second-two-step", source_range(32, 64))
        ],
        "mutation_count": 2,
        "mutations": [
            {"mutation_id": "missing-edge", "kind": "remove_expected_edge", "path_id": "rust-two-step"},
            {"mutation_id": "ambiguous-edge", "kind": "add_ambiguous_edge", "path_id": "rust-second-two-step"}
        ],
        "methodology_sha256": "d".repeat(64),
        "created_at": "2026-08-21T00:00:00Z"
    })
}

fn thresholds() -> Value {
    json!({
        "schema": "codestory.proof-availability-thresholds/v1",
        "thresholds_id": "availability-thresholds-v1",
        "hard_gates": {
            "all_corpus_sources_materialized": true,
            "all_oracle_ranges_verified": true,
            "no_unclassified_failures": true
        },
        "experimental": {"minimum_proven_step_ratio_milli": 500, "minimum_actionable_partial_ratio_milli": 250},
        "stable_explicit": {"minimum_proven_path_ratio_milli": 750, "minimum_proven_step_ratio_milli": 900},
        "automatic": {"minimum_proven_path_ratio_milli": 950},
        "methodology_sha256": "d".repeat(64)
    })
}

fn report() -> Value {
    json!({
        "schema": "codestory.proof-availability-report/v1",
        "corpus_sha256": "a".repeat(64),
        "thresholds_sha256": "b".repeat(64),
        "environment": {"environment_id": "macos", "os": "macos", "architecture": "aarch64", "codestory_version": "0.17.4", "command_sha256": "c".repeat(64)},
        "inventory": [
            {"source_id": "complete", "status": "complete", "observed_files": 1, "source_hashes_verified": 1},
            {"source_id": "incomplete", "status": "incomplete", "observed_files": 1, "source_hashes_verified": 0},
            {"source_id": "unavailable", "status": "unavailable", "observed_files": 0, "source_hashes_verified": 0}
        ],
        "cases": [
            {"case_id": "passed", "source_id": "rust-fixture", "disposition": "passed", "trail": {"path_id": "rust-two-step", "outcome": "proven", "proven_step_count": 2, "observed_step_count": 2, "first_failure_gate": null}, "measurement_bytes": 42},
            {"case_id": "hard", "source_id": "rust-fixture", "disposition": "failed_hard_gate", "trail": {"path_id": "hard", "outcome": "partial", "proven_step_count": 1, "observed_step_count": 2, "first_failure_gate": "source_binding"}, "measurement_bytes": null},
            {"case_id": "experimental", "source_id": "rust-fixture", "disposition": "failed_experimental", "trail": {"path_id": "experimental", "outcome": "rejected", "proven_step_count": 0, "observed_step_count": 2, "first_failure_gate": "raw_admission"}, "measurement_bytes": null},
            {"case_id": "stable", "source_id": "rust-fixture", "disposition": "failed_stable", "trail": {"path_id": "stable", "outcome": "unavailable", "proven_step_count": 0, "observed_step_count": 0, "first_failure_gate": "runtime"}, "measurement_bytes": null},
            {"case_id": "dependency", "source_id": "rust-fixture", "disposition": "dependency_blocked", "trail": null, "measurement_bytes": null}
        ],
        "failure_funnel": {"buckets": [{"gate": "raw_admission", "expected_steps": 10, "failed_steps": 2}], "unclassified_failures": 0},
        "decision": {"outcome": "keep_proof_dark", "rationale": "fixture", "automatic_thresholds_met": false}
    })
}

#[test]
fn maximal_corpus_and_threshold_fixtures_are_closed_and_validated() {
    let corpus: CorpusV1 = serde_json::from_value(corpus()).expect("maximal corpus parses");
    corpus.validate().expect("maximal corpus validates");
    let thresholds: ThresholdsV1 =
        serde_json::from_value(thresholds()).expect("maximal thresholds parse");
    thresholds.validate().expect("maximal thresholds validate");
    let _: QualificationSummaryV1 =
        serde_json::from_value(report()).expect("maximal report parses");

    for document in [
        SchemaDocument::Corpus,
        SchemaDocument::Path,
        SchemaDocument::Report,
        SchemaDocument::Thresholds,
    ] {
        let schema = contracts::schema_json(document);
        assert!(schema.is_object(), "{document:?} schema must be an object");
    }
}

#[test]
fn maximal_fixtures_cover_every_closed_variant() {
    for outcome in ["proven", "partial", "rejected"] {
        let mut fixture = oracle_path("all-path-outcomes", source_range(0, 32));
        fixture["expected_outcome"] = json!(outcome);
        let _: OraclePathV1 = serde_json::from_value(fixture).expect("path outcome parses");
    }
    for fixture in report()["inventory"].as_array().expect("inventory") {
        let _: InventoryReportV1 =
            serde_json::from_value(fixture.clone()).expect("inventory variant parses");
    }
    for fixture in report()["cases"].as_array().expect("cases") {
        let case: CaseReportV1 =
            serde_json::from_value(fixture.clone()).expect("case variant parses");
        if let Some(trail) = case.trail {
            let _: TrailReportV1 =
                serde_json::from_value(serde_json::to_value(trail).expect("trail json"))
                    .expect("trail variant parses");
        }
    }
    for outcome in [
        "public_exact_verifier",
        "experimental_manual_verifier",
        "keep_proof_dark",
        "delay_full_v3_cut",
    ] {
        let mut fixture = report()["decision"].clone();
        fixture["outcome"] = json!(outcome);
        let _: ActivationDecisionV1 =
            serde_json::from_value(fixture).expect("activation outcome parses");
    }
}

#[test]
fn contracts_reject_unknown_fields_and_invalid_source_identity() {
    let mut unknown = corpus();
    unknown["unexpected"] = json!(true);
    assert!(serde_json::from_value::<CorpusV1>(unknown).is_err());

    let mut bad_commit = corpus();
    bad_commit["sources"][0]["commit"] = json!("not-a-commit");
    let bad_commit: CorpusV1 = serde_json::from_value(bad_commit).expect("shape parses");
    assert!(bad_commit.validate().is_err());

    let mut missing_hashes = corpus();
    missing_hashes["sources"][0]["source_hashes"] = json!([]);
    let missing_hashes: CorpusV1 = serde_json::from_value(missing_hashes).expect("shape parses");
    assert!(missing_hashes.validate().is_err());
}

#[test]
fn corpus_rejects_duplicate_ids_wrong_path_and_mutation_counts_and_out_of_bounds_ranges() {
    let mut duplicate = corpus();
    duplicate["paths"][1]["path_id"] = json!("rust-two-step");
    let duplicate: CorpusV1 = serde_json::from_value(duplicate).expect("shape parses");
    assert!(duplicate.validate().is_err());

    let mut wrong_path_count = corpus();
    wrong_path_count["paths"]
        .as_array_mut()
        .expect("paths")
        .pop();
    let wrong_path_count: CorpusV1 =
        serde_json::from_value(wrong_path_count).expect("shape parses");
    assert!(wrong_path_count.validate().is_err());

    let mut wrong_mutation_count = corpus();
    wrong_mutation_count["mutations"]
        .as_array_mut()
        .expect("mutations")
        .pop();
    let wrong_mutation_count: CorpusV1 =
        serde_json::from_value(wrong_mutation_count).expect("shape parses");
    assert!(wrong_mutation_count.validate().is_err());

    let mut out_of_bounds = oracle_path("out-of-bounds", source_range(100, 129));
    let out_of_bounds: OraclePathV1 =
        serde_json::from_value(out_of_bounds.take()).expect("shape parses");
    assert!(out_of_bounds.validate_against_source(128).is_err());
}

#[test]
fn cli_exposes_only_the_three_qualification_commands_without_executing_them() {
    let materialize = cli::Cli::try_parse_from([
        "codestory-proof-availability",
        "materialize",
        "--corpus",
        "/tmp/corpus.json",
    ])
    .expect("materialize parses");
    assert!(matches!(materialize.command, cli::Command::Materialize(_)));
    let run = cli::Cli::try_parse_from([
        "codestory-proof-availability",
        "run",
        "--corpus",
        "/tmp/corpus.json",
        "--thresholds",
        "/tmp/thresholds.json",
        "--output",
        "/tmp/out",
    ])
    .expect("run parses");
    assert!(matches!(run.command, cli::Command::Run(_)));
    let verify = cli::Cli::try_parse_from([
        "codestory-proof-availability",
        "verify",
        "--corpus",
        "/tmp/corpus.json",
        "--thresholds",
        "/tmp/thresholds.json",
    ])
    .expect("verify parses");
    assert!(matches!(verify.command, cli::Command::Verify(_)));
}

#[test]
fn checked_in_schemas_match_the_generated_root_documents() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/proof-availability/schemas");
    for (name, document) in [
        ("corpus.schema.json", SchemaDocument::Corpus),
        ("path.schema.json", SchemaDocument::Path),
        ("report.schema.json", SchemaDocument::Report),
        ("thresholds.schema.json", SchemaDocument::Thresholds),
    ] {
        let checked_in: Value =
            serde_json::from_slice(&std::fs::read(root.join(name)).expect("checked-in schema"))
                .expect("schema JSON");
        assert_eq!(
            checked_in,
            contracts::schema_json(document),
            "schema parity for {name}"
        );
    }
}
