#[path = "../src/bin/codestory_proof_availability/cli.rs"]
mod cli;
#[path = "../src/bin/codestory_proof_availability/contracts.rs"]
mod contracts;

use clap::Parser;
use contracts::{CorpusV1, QualificationSummaryV1, SchemaDocument, ThresholdsV1};
use serde_json::{Value, json};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn range(start: u64, end: u64) -> Value {
    json!({"path":"src/lib.rs","start_byte":start,"end_byte":end,"sha256":SHA})
}

fn path(case_id: &str, cohort: &str) -> Value {
    json!({
      "schema":"codestory.proof-availability-path/v1", "case_id":case_id, "repository_id":cohort, "language":"rust", "source_text":"exact direct ordered call path",
      "clauses":[{"clause_id":"c1","text":"start calls target","range":range(0,20)}],
      "spec":{"start":"crate::start","targets":["crate::target"],"expected_step_count":1},
      "oracle_steps":[{"caller":{"symbol":"crate::start","range":range(0,10)},"callsite":range(11,19),"target":{"symbol":"crate::target","range":range(20,32)}}],
      "negative_mutations":[
        {"mutation_id":format!("{case_id}-missing"),"kind":"remove_expected_relation","step_index":0,"caller":"crate::start","target":"crate::target"},
        {"mutation_id":format!("{case_id}-ambiguous"),"kind":"add_ambiguous_relation","step_index":0,"caller":"crate::start","target":"crate::target"}],
      "audit":{"cohort_path_file":format!("paths/{cohort}.json"),"cohort_path_file_sha256":SHA,"source_tree_sha256":SHA,"source_area":"runtime","curator":"curator@example.invalid","reviewer":"reviewer@example.invalid","review_date":"2026-08-21"}
    })
}

fn corpus() -> Value {
    let ids = ["codestory-rust", "vite-ts-js", "flask-python", "gin-go"];
    json!({
      "schema":"codestory.proof-availability-corpus/v1","corpus_id":"proof-availability-v1","thresholds_sha256":SHA,"methodology_sha256":SHA,"curator":"curator@example.invalid","reviewer":"reviewer@example.invalid","review_date":"2026-08-21",
      "cohorts":ids.iter().map(|id|json!({"repository_id":id,"repository":format!("https://example.invalid/{id}.git"),"commit":COMMIT,"workspace":".","path_file":format!("paths/{id}.json"),"path_file_sha256":SHA,"source_tree_sha256":SHA,"path_count":30,"positive_step_count":78})).collect::<Vec<_>>(),
      "paths":ids.iter().map(|id|path(&format!("{id}-path"),id)).collect::<Vec<_>>(),"positive_request_count":120,"positive_step_count":312,"negative_request_count":240
    })
}

#[allow(clippy::too_many_arguments)]
fn role(
    full: u16,
    cohort: u16,
    recall: u16,
    partial: u16,
    gap: u16,
    unknown_ms: u64,
    transport_ms: u64,
    complete: u64,
    unknown: u64,
) -> Value {
    json!({"minimum_full_proofs":full,"minimum_full_proofs_per_cohort":cohort,"minimum_full_proof_wilson_lower_milli":if full==96{720}else if full==60{410}else{140},"minimum_cohort_wilson_lower_milli":if full==96{500}else if full==60{240}else{0},"minimum_positive_step_recall_milli":recall,"minimum_full_or_useful_partial_milli":partial,"minimum_actionable_exact_gap_milli":gap,"maximum_unknown_p95_ms":unknown_ms,"maximum_transport_p95_ms":transport_ms,"maximum_complete_response_p95_bytes":complete,"maximum_unknown_response_p95_bytes":unknown,"maximum_response_bytes":65536})
}

fn thresholds() -> Value {
    json!({"schema":"codestory.proof-availability-thresholds/v1","thresholds_id":"proof-availability-v1","corpus_sha256":SHA,"methodology_sha256":SHA,"wilson_z":1.959963984540054,"expected_cohort_count":4,"expected_positive_requests":120,"expected_positive_steps":312,"expected_negative_requests":240,"hard_gates":{"maximum_false_contract_proven":0,"require_exact_receipt_matches":true,"maximum_certified_absence":0,"require_complete_failure_funnel":true,"require_complete_provenance":true,"maximum_proof_bytes":65536,"require_each_cohort":true,"require_product_disposition_match":true},"automatic":role(96,21,900,950,950,500,1500,32768,16384),"stable_explicit":role(60,12,750,800,900,1000,2000,32768,16384),"experimental":role(24,12,500,600,800,2000,3000,49152,24576)})
}

fn report() -> Value {
    json!({
      "schema":"codestory.proof-availability-report/v1","qualification_id":"20260821T000000Z-0123456789ab",
      "provenance":{"source_commit":COMMIT,"source_tree":COMMIT,"binary_sha256":SHA,"corpus_sha256":SHA,"thresholds_sha256":SHA,"results_sha256":SHA},
      "environment":{"environment_id":"macos-arm64","os":"macos","architecture":"aarch64","rust_host":"aarch64-apple-darwin","binary_sha256":SHA,"core_generation":1,"core_run_id":"run-1","database_sha256":SHA},
      "inventory":[{"repository_id":"codestory-rust","stored_call_rows":10,"effective_endpoint_rows":10,"exact_resolved_rows":8,"admitted_rows":7,"unresolved_placeholder_rows":2}],
      "trails":[{"repository_id":"codestory-rust","lengths":[{"length":1,"effective_endpoint":10,"exact_resolved":8,"strictly_admitted":7},{"length":2,"effective_endpoint":9,"exact_resolved":7,"strictly_admitted":6},{"length":3,"effective_endpoint":8,"exact_resolved":6,"strictly_admitted":5},{"length":4,"effective_endpoint":7,"exact_resolved":5,"strictly_admitted":4},{"length":5,"effective_endpoint":6,"exact_resolved":4,"strictly_admitted":3},{"length":6,"effective_endpoint":5,"exact_resolved":3,"strictly_admitted":2}]}],
      "cases":[{"case_id":"codestory-rust-path","repository_id":"codestory-rust","product_disposition":{"kind":"contract_proven","gaps":[]},"authoritative_receipt_count":1,"oracle_receipts_exact":true,"proven_step_precision_milli":1000,"proven_step_recall_milli":1000,"proven_prefix_length":1,"actionable_exact_gap":null,"diagnostic_candidate_count":0,"authoritative_receipt_evidence_count":1,"warm_end_to_end_ms":12,"stage_durations_ms":{"validation":1,"operation":2},"complete_projection_bytes":128,"tool_result_bytes":{"v2024_11_05":128,"v2025_03_26":128,"v2025_06_18":128,"v2025_11_25":128},"negative_mutations":[{"mutation_id":"negative-1","contract_proven":false},{"mutation_id":"negative-2","contract_proven":false}],"first_failure":{"kind":"admitted","edge_ids":[1],"histogram":[],"finalization":null}}],
      "failure_funnel":{"attempted_positive_steps":312,"classified_positive_steps":312,"unclassified_positive_steps":0,"buckets":[{"failure":{"kind":"raw_admission","reason":"certainty_probable","edge_ids":[9],"histogram":[{"reason":"certainty_probable","edge_ids":[9]}],"finalization":null},"count":1}]},
      "decision":{"outcome":"keep_proof_dark","failed_gates":[{"kind":"experimental_usefulness","detail":"below threshold"}],"automatic_thresholds_met":false}
    })
}

#[test]
fn frozen_corpus_has_all_oracle_freeze_inputs_and_rejects_semantic_violations() {
    CorpusV1::from_json(corpus()).expect("maximal frozen corpus");
    let mut unknown = corpus();
    unknown["unknown"] = json!(true);
    assert!(CorpusV1::from_json(unknown).is_err());
    let mut commit = corpus();
    commit["cohorts"][0]["commit"] = json!("abc");
    assert!(CorpusV1::from_json(commit).is_err());
    let mut hash = corpus();
    hash["paths"][0]["oracle_steps"][0]["callsite"]["sha256"] = json!("");
    assert!(CorpusV1::from_json(hash).is_err());
    let mut mutations = corpus();
    mutations["paths"][0]["negative_mutations"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(CorpusV1::from_json(mutations).is_err());
}

#[test]
fn frozen_thresholds_cover_all_role_and_hard_gate_semantics() {
    ThresholdsV1::from_json(thresholds()).expect("maximal thresholds");
    let mut invalid = thresholds();
    invalid["hard_gates"]["maximum_proof_bytes"] = json!(65537);
    assert!(ThresholdsV1::from_json(invalid).is_err());
    let mut invalid = thresholds();
    invalid["automatic"]["minimum_full_proof_wilson_lower_milli"] = json!(1001);
    assert!(ThresholdsV1::from_json(invalid).is_err());
}

#[test]
fn reports_preserve_typed_task_8_to_13_evidence_and_reject_open_gates() {
    QualificationSummaryV1::from_json(report()).expect("maximal report");
    let mut lengths = report();
    lengths["trails"][0]["lengths"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(QualificationSummaryV1::from_json(lengths).is_err());
    let mut reason = report();
    reason["failure_funnel"]["buckets"][0]["failure"]["reason"] = json!("free form");
    assert!(QualificationSummaryV1::from_json(reason).is_err());
    let mut gate = report();
    gate["decision"]["failed_gates"][0]["kind"] = json!("free form");
    assert!(QualificationSummaryV1::from_json(gate).is_err());
}

#[test]
fn schemas_have_semantic_constants_patterns_and_bounds() {
    for document in [
        SchemaDocument::Corpus,
        SchemaDocument::Path,
        SchemaDocument::Report,
        SchemaDocument::Thresholds,
    ] {
        let schema = contracts::schema_json(document);
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(schema.to_string().contains("^[0-9a-f]{64}$"));
    }
    assert!(
        contracts::schema_json(SchemaDocument::Corpus)
            .to_string()
            .contains("^[0-9a-f]{40}$")
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Corpus)["properties"]["schema"]["const"],
        "codestory.proof-availability-corpus/v1"
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Path)["properties"]["negative_mutations"]["minItems"],
        2
    );
}

#[test]
fn cli_matches_frozen_materialize_run_and_verify_shapes() {
    assert!(matches!(
        cli::Cli::try_parse_from([
            "bin",
            "materialize",
            "--corpus",
            "/tmp/c",
            "--workspace",
            "/tmp/w",
            "--cache-root",
            "/tmp/cache",
            "--out",
            "/tmp/e",
            "--verify-only"
        ])
        .expect("materialize")
        .command,
        cli::Command::Materialize(_)
    ));
    assert!(matches!(
        cli::Cli::try_parse_from([
            "bin",
            "run",
            "--corpus",
            "/tmp/c",
            "--environment",
            "/tmp/e",
            "--out",
            "/tmp/r"
        ])
        .expect("run")
        .command,
        cli::Command::Run(_)
    ));
    assert!(matches!(
        cli::Cli::try_parse_from([
            "bin",
            "verify",
            "--thresholds",
            "/tmp/t",
            "--results",
            "/tmp/r"
        ])
        .expect("verify")
        .command,
        cli::Command::Verify(_)
    ));
    assert!(
        cli::Cli::try_parse_from(["bin", "run", "--thresholds", "/tmp/t", "--output", "/tmp/o"])
            .is_err()
    );
}

#[test]
fn checked_in_schemas_match_generated_contracts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/proof-availability/schemas");
    for (name, document) in [
        ("corpus.schema.json", SchemaDocument::Corpus),
        ("path.schema.json", SchemaDocument::Path),
        ("report.schema.json", SchemaDocument::Report),
        ("thresholds.schema.json", SchemaDocument::Thresholds),
    ] {
        let checked: Value =
            serde_json::from_slice(&std::fs::read(root.join(name)).expect("schema")).expect("json");
        assert_eq!(checked, contracts::schema_json(document), "{name}");
    }
}

#[test]
#[ignore = "explicit checked-in schema regeneration"]
fn write_checked_in_schemas() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/proof-availability/schemas");
    std::fs::create_dir_all(&root).expect("schema dir");
    for (name, document) in [
        ("corpus.schema.json", SchemaDocument::Corpus),
        ("path.schema.json", SchemaDocument::Path),
        ("report.schema.json", SchemaDocument::Report),
        ("thresholds.schema.json", SchemaDocument::Thresholds),
    ] {
        let rendered =
            serde_json::to_string_pretty(&contracts::schema_json(document)).expect("render");
        std::fs::write(root.join(name), format!("{rendered}\n")).expect("write");
    }
}
