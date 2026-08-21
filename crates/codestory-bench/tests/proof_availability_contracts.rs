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
    json!({"path":"src/lib.rs","start_byte":start,"end_byte":end,"file_byte_length":4096,"sha256":SHA})
}

fn path(case_id: &str, cohort: &str, step_count: u8) -> Value {
    let oracle_steps = (0..step_count)
        .map(|index| {
            let start = u64::from(index) * 40;
            let caller_symbol = if index == 0 {
                "crate::start".to_owned()
            } else {
                format!("crate::target_{}", index - 1)
            };
            let caller_range = if index == 0 {
                range(0, 10)
            } else {
                range(start - 20, start - 8)
            };
            json!({
              "caller":{"symbol":caller_symbol,"range":caller_range},
              "callsite":range(start + 11, start + 19),
              "target":{"symbol":format!("crate::target_{index}"),"range":range(start + 20, start + 32)}
            })
        })
        .collect::<Vec<_>>();
    let targets = (0..step_count)
        .map(|index| format!("crate::target_{index}"))
        .collect::<Vec<_>>();
    json!({
      "schema":"codestory.proof-availability-path/v1", "case_id":case_id, "repository_id":cohort, "language":"rust", "source_text":"exact direct ordered call path",
      "clauses":[{"clause_id":"c1","text":"start calls target","range":range(0,20)}],
      "spec":{"start":"crate::start","targets":targets,"expected_step_count":step_count},
      "oracle_steps":oracle_steps,
      "negative_mutations":[
        {"mutation_id":format!("{case_id}-missing"),"path_id":case_id,"kind":"remove_expected_relation","step_index":0,"caller":"crate::start","target":"crate::target_0"},
        {"mutation_id":format!("{case_id}-ambiguous"),"path_id":case_id,"kind":"add_ambiguous_relation","step_index":0,"caller":"crate::start","target":"crate::target_0"}],
      "audit":{"cohort_path_file":format!("paths/{cohort}.json"),"cohort_path_file_sha256":SHA,"source_tree_sha256":SHA,"source_area":"runtime","curator":"curator@example.invalid","reviewer":"reviewer@example.invalid","review_date":"2026-08-21"}
    })
}

fn corpus() -> Value {
    let ids = ["codestory-rust", "vite-ts-js", "flask-python", "gin-go"];
    // Thirty paths and 78 steps per cohort; across four cohorts this is the
    // frozen 120-path / 312-step / 240-mutation corpus.
    let lengths = [10u8, 7, 5, 3, 3, 2];
    let paths = ids
        .iter()
        .flat_map(|id| {
            lengths.iter().enumerate().flat_map(move |(length, count)| {
                (0..*count).map(move |index| {
                    path(
                        &format!("{id}-l{}-{index}", length + 1),
                        id,
                        (length + 1) as u8,
                    )
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
      "schema":"codestory.proof-availability-corpus/v1","corpus_id":"proof-availability-v1","thresholds_sha256":SHA,"methodology_sha256":SHA,"curator":"curator@example.invalid","reviewer":"reviewer@example.invalid","review_date":"2026-08-21",
      "cohorts":ids.iter().map(|id|json!({"repository_id":id,"repository":format!("https://example.invalid/{id}.git"),"commit":COMMIT,"workspace":".","path_file":format!("paths/{id}.json"),"path_file_sha256":SHA,"source_tree_sha256":SHA,"path_count":30,"positive_step_count":78})).collect::<Vec<_>>(),
      "paths":paths,"positive_request_count":120,"positive_step_count":312,"negative_request_count":240
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
    json!({"schema":"codestory.proof-availability-thresholds/v1","thresholds_id":"proof-availability-v1","corpus_sha256":SHA,"methodology_sha256":SHA,"wilson_z":1.959963984540054,"expected_cohort_count":4,"expected_positive_requests":120,"expected_positive_steps":312,"expected_negative_requests":240,"hard_gates":{"maximum_false_contract_proven":0,"require_exact_receipt_matches":true,"maximum_certified_absence":0,"require_complete_failure_funnel":true,"require_complete_provenance":true,"maximum_invalid_results":0,"maximum_over_cap_results":0,"maximum_transport_errors":0,"maximum_proof_bytes":65536,"require_each_cohort":true,"require_product_disposition_match":true},"automatic":role(96,21,900,950,950,500,1500,32768,16384),"stable_explicit":role(60,12,750,800,900,1000,2000,32768,16384),"experimental":role(24,12,500,600,800,2000,3000,49152,24576)})
}

fn report() -> Value {
    json!({
      "schema":"codestory.proof-availability-report/v1","qualification_id":"20260821T000000Z-0123456789ab",
      "provenance":{"source_commit":COMMIT,"source_tree":COMMIT,"binary_sha256":SHA,"corpus_sha256":SHA,"thresholds_sha256":SHA,"results_sha256":SHA},
      "environment":{"environment_id":"macos-arm64","os":"macos","architecture":"aarch64","rust_host":"aarch64-apple-darwin","binary_sha256":SHA,"core_generation":1,"core_run_id":"run-1","database_sha256":SHA},
      "inventory":[{"repository_id":"codestory-rust","stored_call_rows":"10","effective_endpoint_rows":"10","exact_resolved_rows":"8","admitted_rows":"7","unresolved_placeholder_rows":"2"}],
      "trails":[{"repository_id":"codestory-rust","lengths":[{"length":1,"effective_endpoint":"10","exact_resolved":"8","strictly_admitted":"7"},{"length":2,"effective_endpoint":"9","exact_resolved":"7","strictly_admitted":"6"},{"length":3,"effective_endpoint":"8","exact_resolved":"6","strictly_admitted":"5"},{"length":4,"effective_endpoint":"7","exact_resolved":"5","strictly_admitted":"4"},{"length":5,"effective_endpoint":"6","exact_resolved":"4","strictly_admitted":"3"},{"length":6,"effective_endpoint":"5","exact_resolved":"3","strictly_admitted":"2"}]}],
      "cases":[{"case_id":"codestory-rust-path","repository_id":"codestory-rust","product_disposition":{"kind":"contract_proven","gaps":[]},"authoritative_receipt_count":1,"oracle_receipts_exact":true,"proven_step_precision_milli":1000,"proven_step_recall_milli":1000,"proven_prefix_length":1,"actionable_exact_gap":null,"diagnostic_candidate_count":0,"authoritative_receipt_evidence_count":1,"warm_end_to_end_ms":12,"stage_durations_ms":{"validation":1,"operation":2},"attempted_step_count":1,"complete_projection_bytes":128,"transport":{"kind":"measurements","measurements":{"measurements":[{"revision":"2024-11-05","actual_bytes":128},{"revision":"2025-03-26","actual_bytes":128},{"revision":"2025-06-18","actual_bytes":128},{"revision":"2025-11-25","actual_bytes":128}]}},"negative_mutations":[{"mutation_id":"negative-1","contract_proven":false},{"mutation_id":"negative-2","contract_proven":false}],"proof_trace":{"selectors":[{"selector_index":0,"outcome":{"kind":"resolved","node_id":1}}],"selector_early_return":false,"steps":[{"step_index":0,"candidate_edge_ids":[1],"outcome":{"kind":"admitted","edge_ids":[1]}}],"finalization":{"kind":"complete","projection_bytes":128}}}],
      "failure_funnel":{"attempted_positive_steps":312,"classified_positive_steps":312,"unclassified_positive_steps":0,"buckets":[{"outcome":{"kind":"admitted"},"count":"311"},{"outcome":{"kind":"first_zero_survivor","gate":"raw_admission","histogram":[{"reason":{"kind":"raw_admission","reason":"certainty_probable"},"edge_ids":[9]}]},"count":"1"}]},
      "decision":{"outcome":"keep_proof_dark","failed_gates":[{"gate_id":"experimental-usefulness-1","kind":"experimental_usefulness","detail":{"kind":"count","observed":"1","required":"24"}}],"automatic_thresholds_met":false}
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
    let mut missing_path = corpus();
    missing_path["paths"].as_array_mut().unwrap().pop();
    assert!(CorpusV1::from_json(missing_path).is_err());
    let mut duplicate_kind = corpus();
    duplicate_kind["paths"][0]["negative_mutations"][1]["kind"] = json!("remove_expected_relation");
    assert!(CorpusV1::from_json(duplicate_kind).is_err());
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
    reason["failure_funnel"]["buckets"][1]["outcome"]["histogram"][0]["reason"]["reason"] =
        json!("free form");
    assert!(QualificationSummaryV1::from_json(reason).is_err());
    let mut gate = report();
    gate["decision"]["failed_gates"][0]["kind"] = json!("free form");
    assert!(QualificationSummaryV1::from_json(gate).is_err());
    let mut hard_failure = report();
    hard_failure["failure_funnel"]["classified_positive_steps"] = json!(311);
    hard_failure["failure_funnel"]["unclassified_positive_steps"] = json!(1);
    hard_failure["failure_funnel"]["buckets"]
        .as_array_mut()
        .unwrap()
        .pop();
    hard_failure["cases"][0]["transport"] = json!({
        "kind":"error",
        "error":{"kind":"result_exceeds_budget","maximum_bytes":65536,"actual_bytes":65537}
    });
    QualificationSummaryV1::from_json(hard_failure).expect("failure evidence is representable");
}

#[test]
fn closed_contracts_reject_hostile_nested_shapes() {
    let mut too_many_steps = corpus();
    too_many_steps["paths"][0]["spec"]["expected_step_count"] = json!(7);
    assert!(CorpusV1::from_json(too_many_steps).is_err());

    let mut invalid_trace = report();
    invalid_trace["cases"][0]["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor",
        "gate":"line",
        "histogram":[{"reason":{"kind":"raw_admission","reason":"certainty_probable"},"edge_ids":[1]}]
    });
    assert!(QualificationSummaryV1::from_json(invalid_trace).is_err());

    let mut invalid_transport = report();
    invalid_transport["cases"][0]["transport"] = json!({
        "kind":"error",
        "error":{"kind":"result_exceeds_budget","maximum_bytes":1,"actual_bytes":65537}
    });
    assert!(QualificationSummaryV1::from_json(invalid_transport).is_err());

    let mut noncanonical_u128 = report();
    noncanonical_u128["inventory"][0]["stored_call_rows"] = json!("01");
    assert!(QualificationSummaryV1::from_json(noncanonical_u128).is_err());

    let mut unknown_variant_field = report();
    unknown_variant_field["cases"][0]["transport"] =
        json!({"kind":"error","error":{"kind":"output_schema_violation","surprise":true}});
    assert!(QualificationSummaryV1::from_json(unknown_variant_field).is_err());

    let mut duplicate_kind = report();
    duplicate_kind["decision"]["failed_gates"] = json!([
        {"gate_id":"one","kind":"cohort_failure","detail":{"kind":"cohort","repository_id":"rust","observed":"1","required":"30"}},
        {"gate_id":"two","kind":"cohort_failure","detail":{"kind":"cohort","repository_id":"typescript","observed":"1","required":"30"}}
    ]);
    QualificationSummaryV1::from_json(duplicate_kind).expect("each failed gate has a stable id");

    let mut delayed_without_evidence = report();
    delayed_without_evidence["decision"]["outcome"] = json!("delay_full_v3_cut");
    assert!(QualificationSummaryV1::from_json(delayed_without_evidence).is_err());
    let mut delayed_with_evidence = report();
    delayed_with_evidence["decision"] = json!({
        "outcome":"delay_full_v3_cut",
        "automatic_thresholds_met":null,
        "failed_gates":[{"gate_id":"packet-proof-dependency","kind":"integration_dependency","detail":{"kind":"source_dependency","evidence":{"source_path":"src/lib.rs","source_range":range(0,10),"source_sha256":SHA,"dependency":"v3_packet_requires_proof","passing_test":{"test_id":"packet_v3_requires_proof","kind":"packet_v3_requires_proof","status":"passed"}}}}]
    });
    QualificationSummaryV1::from_json(delayed_with_evidence)
        .expect("outcome D uses closed source evidence");
    let mut wrong_dependency_gate = report();
    wrong_dependency_gate["decision"]["failed_gates"][0]["detail"] = json!({
        "kind":"source_dependency",
        "evidence":{"source_path":"src/lib.rs","source_range":range(0,10),"source_sha256":SHA,"dependency":"v3_packet_requires_proof","passing_test":{"test_id":"packet_v3_requires_proof","kind":"packet_v3_requires_proof","status":"passed"}}
    });
    assert!(QualificationSummaryV1::from_json(wrong_dependency_gate).is_err());
}

#[test]
fn producer_mapping_red_requires_ordered_oracles_and_lossless_task4_errors() {
    let mut wrong_target_order = corpus();
    wrong_target_order["paths"][0]["spec"]["targets"] = json!(["crate::wrong_target"]);
    assert!(CorpusV1::from_json(wrong_target_order).is_err());

    let mut task4_serialization = report();
    task4_serialization["cases"][0]["transport"] = json!({
        "kind":"error",
        "error":{"kind":"serialization","message":"exact encoder failure"}
    });
    QualificationSummaryV1::from_json(task4_serialization)
        .expect("Task 4 producer error must remain representable");
}

#[test]
fn invariant_table_exhausts_task4_task6_and_corpus_variants() {
    for error in [
        json!({"kind":"serialization","message":"encode failed"}),
        json!({"kind":"invalid_projection","projection":"root"}),
        json!({"kind":"output_schema_violation"}),
        json!({"kind":"result_exceeds_budget","maximum_bytes":65536,"actual_bytes":65537}),
    ] {
        let mut value = report();
        value["cases"][0]["transport"] = json!({"kind":"error","error":error});
        QualificationSummaryV1::from_json(value).expect("every Task 4 error maps losslessly");
    }

    for reason in [
        "wrong_kind",
        "certainty_absent",
        "certainty_probable",
        "certainty_uncertain",
        "wrong_effective_source",
        "wrong_effective_target",
        "missing_exact_resolved_target",
        "candidate_alternatives_retained",
        "missing_file_node",
        "missing_line",
        "invalid_or_legacy_callsite_identity",
        "callsite_file_mismatch",
        "callsite_line_mismatch",
        "callsite_raw_target_mismatch",
    ] {
        assert_valid_first_zero("raw_admission", "raw_admission", reason);
    }
    for reason in ["edge_source_file_mismatch", "missing", "ambiguous"] {
        assert_valid_first_zero("containment", "containment", reason);
    }
    for reason in [
        "file_incomplete",
        "stored_hash_absent",
        "working_tree_read_failed",
        "working_tree_hash_mismatch",
        "invalid_utf8",
    ] {
        assert_valid_first_zero("source_binding", "source_binding", reason);
    }
    for reason in ["line_missing", "line_over_limit"] {
        assert_valid_first_zero("line", "source_binding", reason);
    }

    for outcome in [
        json!({"kind":"failed","reason":"missing"}),
        json!({"kind":"failed","reason":"ambiguous"}),
        json!({"kind":"failed","reason":"non_callable"}),
        json!({"kind":"unavailable","reason":"validated_contract_hash_mismatch"}),
        json!({"kind":"unavailable","reason":"publication_pin_mismatch"}),
        json!({"kind":"unavailable","reason":"source_not_bound_to_publication"}),
        json!({"kind":"unavailable","reason":"proof_facts_unavailable"}),
    ] {
        let mut value = report();
        value["cases"][0]["attempted_step_count"] = json!(0);
        value["cases"][0]["proof_trace"] = json!({
            "selectors":[{"selector_index":0,"outcome":outcome}],
            "selector_early_return":true,
            "steps":[],
            "finalization":{"kind":"not_run"}
        });
        QualificationSummaryV1::from_json(value).expect("every selector outcome maps losslessly");
    }

    for failure in ["receipt_integration", "receipt_budget", "projection_budget"] {
        let mut value = report();
        value["cases"][0]["proof_trace"]["finalization"] =
            json!({"kind":"failed","failure":failure});
        QualificationSummaryV1::from_json(value)
            .expect("every finalization failure maps losslessly");
    }

    let mut wrong_chain = corpus();
    wrong_chain["paths"][10]["oracle_steps"][1]["caller"]["symbol"] = json!("broken");
    assert!(CorpusV1::from_json(wrong_chain).is_err());
    let mut wrong_target_count = corpus();
    wrong_target_count["paths"][0]["spec"]["targets"] =
        json!(["crate::target_0", "crate::target_1"]);
    assert!(CorpusV1::from_json(wrong_target_count).is_err());
    let mut wrong_mutation = corpus();
    wrong_mutation["paths"][0]["negative_mutations"][0]["path_id"] = json!("other");
    assert!(CorpusV1::from_json(wrong_mutation).is_err());
    let mut wrong_range = corpus();
    wrong_range["paths"][0]["oracle_steps"][0]["callsite"]["end_byte"] = json!(4097);
    assert!(CorpusV1::from_json(wrong_range).is_err());
}

fn assert_valid_first_zero(gate: &str, kind: &str, reason: &str) {
    let mut value = report();
    value["cases"][0]["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor",
        "gate":gate,
        "histogram":[{"reason":{"kind":kind,"reason":reason},"edge_ids":[1]}]
    });
    QualificationSummaryV1::from_json(value).expect("mapped first-zero outcome");
}

#[test]
fn invariant_table_exercises_remaining_closed_decision_and_funnel_variants() {
    for disposition in ["unknown", "certified_absence", "invalid"] {
        let mut value = report();
        value["cases"][0]["product_disposition"] = json!({"kind":disposition,"gaps":[]});
        QualificationSummaryV1::from_json(value).expect("closed product disposition");
    }
    for gap in [
        "selector_missing",
        "selector_ambiguous",
        "relation_missing",
        "recursion",
        "source_binding",
        "projection_budget",
    ] {
        let mut value = report();
        value["cases"][0]["product_disposition"] = json!({"kind":"unknown","gaps":[gap]});
        QualificationSummaryV1::from_json(value).expect("closed actionable gap");
    }
    for gate in [
        "false_contract_proven",
        "receipt_mismatch",
        "certified_absence",
        "failure_funnel",
        "provenance",
        "response_size",
        "cohort_failure",
        "product_disposition_mismatch",
        "automatic_threshold",
        "stable_threshold",
        "experimental_usefulness",
    ] {
        let mut value = report();
        value["decision"]["failed_gates"][0]["kind"] = json!(gate);
        QualificationSummaryV1::from_json(value).expect("closed qualification gate");
    }
    for outcome in [
        "public_exact_verifier",
        "experimental_manual_verifier",
        "keep_proof_dark",
    ] {
        let mut value = report();
        value["decision"]["outcome"] = json!(outcome);
        QualificationSummaryV1::from_json(value).expect("closed activation outcome");
    }
    let mut selector_funnel = report();
    selector_funnel["failure_funnel"]["buckets"] = json!([
        {"outcome":{"kind":"selector_early_return","outcome":{"kind":"failed","reason":"missing"}},"count":"312"}
    ]);
    QualificationSummaryV1::from_json(selector_funnel).expect("selector funnel outcome");

    let mut transport_dependency = report();
    transport_dependency["decision"] = json!({
        "outcome":"delay_full_v3_cut",
        "automatic_thresholds_met":null,
        "failed_gates":[{"gate_id":"transport-keep-dark-dependency","kind":"integration_dependency","detail":{"kind":"source_dependency","evidence":{"source_path":"src/lib.rs","source_range":range(0,10),"source_sha256":SHA,"dependency":"transport_cannot_represent_keep_dark","passing_test":{"test_id":"transport_cannot_represent_keep_dark","kind":"transport_cannot_represent_keep_dark","status":"passed"}}}}]
    });
    QualificationSummaryV1::from_json(transport_dependency)
        .expect("second source dependency variant");
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
    assert_eq!(
        contracts::schema_json(SchemaDocument::Corpus)["properties"]["paths"]["minItems"],
        120
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Thresholds)["$defs"]["RoleThresholdsV1"]["properties"]
            ["minimum_positive_step_recall_milli"]["maximum"],
        1000
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Path)["$defs"]["CallPathSpecV1"]["properties"]["targets"]
            ["maxItems"],
        6
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["FailureFunnelReportV1"]["properties"]
            ["classified_positive_steps"]["maximum"],
        312
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["TransportMeasurementV1"]["properties"]
            ["actual_bytes"]["maximum"],
        65536
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
