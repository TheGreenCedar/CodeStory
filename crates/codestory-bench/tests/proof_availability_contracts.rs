#[path = "../src/bin/codestory_proof_availability/cli.rs"]
mod cli;
#[path = "../src/bin/codestory_proof_availability/contracts.rs"]
mod contracts;

use clap::Parser;
use contracts::{
    ActivationDecisionV1, CandidateFailureV1, CandidateGateV1, CorpusV1, FinalizationTraceV1,
    FunnelOutcomeV1, MAX_CANDIDATE_EDGES_PER_STEP, MAX_OBSERVED_RECEIPTS_PER_CASE,
    ObservedReceiptV1, ProofQualificationTraceV1, QualificationSummaryV1,
    ReceiptOracleComparisonV1, SchemaDocument, SelectorGateOutcomeV1, ThresholdsV1,
    TransportEvidenceV1, canonical_corpus_sha256, canonical_thresholds_sha256,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
              "callsite_line":index + 1,
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
    let threshold_hash =
        canonical_thresholds_sha256(&ThresholdsV1::from_json(thresholds()).expect("thresholds"))
            .expect("canonical thresholds hash");
    json!({
      "schema":"codestory.proof-availability-corpus/v1","corpus_id":"proof-availability-v1","thresholds_sha256":threshold_hash,"methodology_sha256":SHA,"curator":"curator@example.invalid","reviewer":"reviewer@example.invalid","review_date":"2026-08-21",
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
    json!({"schema":"codestory.proof-availability-thresholds/v1","thresholds_id":"proof-availability-v1","methodology_sha256":SHA,"wilson_z":1.959963984540054,"expected_cohort_count":4,"expected_positive_requests":120,"expected_positive_steps":312,"expected_negative_requests":240,"hard_gates":{"maximum_false_contract_proven":0,"require_exact_receipt_matches":true,"maximum_certified_absence":0,"require_complete_failure_funnel":true,"require_complete_provenance":true,"maximum_invalid_results":0,"maximum_over_cap_results":0,"maximum_transport_errors":0,"maximum_proof_bytes":65536,"require_each_cohort":true,"require_product_disposition_match":true},"automatic":role(96,21,900,950,950,500,1500,32768,16384),"stable_explicit":role(60,12,750,800,900,1000,2000,32768,16384),"experimental":role(24,12,500,600,800,2000,3000,49152,24576)})
}

fn report() -> Value {
    let frozen = corpus();
    let threshold_hash = frozen["thresholds_sha256"]
        .as_str()
        .expect("threshold hash")
        .to_owned();
    let corpus_hash =
        canonical_corpus_sha256(&CorpusV1::from_json(frozen.clone()).expect("corpus"))
            .expect("canonical corpus hash");
    let cohort_ids = frozen["cohorts"]
        .as_array()
        .expect("cohorts")
        .iter()
        .map(|cohort| cohort["repository_id"].as_str().expect("repository id"))
        .collect::<Vec<_>>();
    let cases = frozen["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .enumerate()
        .map(|(case_index, path)| {
            let attempted = path["spec"]["expected_step_count"].as_u64().expect("step count");
            let case_id = path["case_id"].as_str().expect("case id");
            let repository_id = path["repository_id"].as_str().expect("repository id");
            let project_id = format!("project-{repository_id}");
            let core_generation_id = format!("generation-{repository_id}");
            let core_run_id = format!("run-{repository_id}");
            let file_node_id = -(i64::try_from(case_index).expect("case index") + 10_000);
            let steps = (0..attempted)
                .map(|step_index| {
                    let edge_id = i64::try_from(case_index * 10 + step_index as usize + 1)
                        .expect("fixture edge id");
                    json!({"step_index":step_index,"candidate_edge_ids":[edge_id],"outcome":{"kind":"admitted","edge_ids":[edge_id]}})
                })
                .collect::<Vec<_>>();
            let selectors = (0..=attempted)
                .map(|selector_index| json!({"selector_index":selector_index,"outcome":{"kind":"resolved","node_id":-(selector_index as i64 + 1)}}))
                .collect::<Vec<_>>();
            let observed_receipts = path["oracle_steps"]
                .as_array()
                .expect("oracle steps")
                .iter()
                .enumerate()
                .map(|(step_index, step)| {
                    let edge_id = i64::try_from(case_index * 10 + step_index + 1)
                        .expect("fixture edge id");
                    let callsite = &step["callsite"];
                    let start = callsite["start_byte"].as_u64().expect("callsite start");
                    let end = callsite["end_byte"].as_u64().expect("callsite end");
                    let source_node_id = -(i64::try_from(step_index).expect("step index") + 1);
                    let target_node_id = source_node_id - 1;
                    json!({
                        "receipt_id":format!("indexed-call-edge:fixture-{case_index}-{step_index}"),
                        "step_index":step_index,
                        "edge_id":edge_id,
                        "source":{
                            "pinned":{"project_id":project_id,"core_generation_id":core_generation_id,"core_run_id":core_run_id,"node_id":source_node_id.to_string()},
                            "canonical_id":format!("canonical-{case_index}-{source_node_id}"),
                            "qualified_name":step["caller"]["symbol"],
                            "project_file_components":["src","lib.rs"]
                        },
                        "target":{
                            "pinned":{"project_id":project_id,"core_generation_id":core_generation_id,"core_run_id":core_run_id,"node_id":target_node_id.to_string()},
                            "canonical_id":format!("canonical-{case_index}-{target_node_id}"),
                            "qualified_name":step["target"]["symbol"],
                            "project_file_components":["src","lib.rs"]
                        },
                        "certainty":"certain",
                        "callsite_identity":format!("{file_node_id}:{}:0:{target_node_id}|fixture", step_index + 1),
                        "callsite_line":step_index + 1,
                        "containment":{"file_node_id":file_node_id,"owner_node_id":source_node_id,"start_line":1,"end_line":attempted},
                        "line_window":{
                            "kind":"indexed_line_v1",
                            "project_file_components":["src","lib.rs"],
                            "byte_start":start,
                            "byte_end":end,
                            "indexed_sha256":SHA,
                            "observed_sha256":SHA,
                            "text":"call();\n"
                        },
                        "oracle_comparison":{"kind":"exact","oracle_step_index":step_index,"oracle_step":step}
                    })
                })
                .collect::<Vec<_>>();
            let authoritative_receipts = observed_receipts
                .iter()
                .map(|receipt| json!({
                    "receipt_id":receipt["receipt_id"],
                    "edge_id":receipt["edge_id"]
                }))
                .collect::<Vec<_>>();
            let negative_mutations = path["negative_mutations"]
                .as_array()
                .expect("mutations")
                .iter()
                .map(|mutation| json!({
                    "mutation_id":mutation["mutation_id"],
                    "path_id":mutation["path_id"],
                    "kind":mutation["kind"],
                    "step_index":mutation["step_index"],
                    "caller":mutation["caller"],
                    "target":mutation["target"],
                    "contract_proven":false
                }))
                .collect::<Vec<_>>();
            json!({
                "case_id":case_id,"repository_id":repository_id,
                "product_disposition":{"kind":"contract_proven","gaps":[],"authoritative_receipts":authoritative_receipts},
                "actionable_exact_gap":null,
                "warm_end_to_end_ms":12,"stage_durations_ms":{"validation":1,"operation":2},
                "attempted_step_count":attempted,"unclassified_step_indices":[],
                "receipt_evidence":{"observed_receipts":observed_receipts,"missing_oracle_steps":[]},
                "complete_projection_bytes":128,
                "transport":{"kind":"measurements","measurements":{"measurements":[
                    {"revision":"2024-11-05","actual_bytes":128,"elapsed_ns":11},
                    {"revision":"2025-03-26","actual_bytes":128,"elapsed_ns":22},
                    {"revision":"2025-06-18","actual_bytes":128,"elapsed_ns":33},
                    {"revision":"2025-11-25","actual_bytes":128,"elapsed_ns":44}
                ]}},
                "negative_mutations":negative_mutations,
                "proof_trace":{"selectors":selectors,"selector_early_return":false,"steps":steps,"finalization":{"kind":"complete","projection_bytes":128}}
            })
        })
        .collect::<Vec<_>>();
    json!({
      "schema":"codestory.proof-availability-report/v1","qualification_id":"20260821T000000Z-0123456789ab",
      "provenance":{"source_commit":COMMIT,"source_tree":COMMIT,"binary_sha256":SHA,"corpus_sha256":corpus_hash,"thresholds_sha256":threshold_hash,"results_sha256":SHA},
      "environment":{"environment_id":"macos-arm64","os":"macos","architecture":"aarch64","rust_host":"aarch64-apple-darwin","binary_sha256":SHA,"projects":cohort_ids.iter().map(|id|json!({"repository_id":id,"source_head":COMMIT,"source_tree":SHA,"store_schema":"codestory-store/v1","file_count":10,"node_count":20,"edge_count":30,"freshness":"fresh","database_sha256":SHA,"core_generation":1,"identity":{"project_id":format!("project-{id}"),"core_generation_id":format!("generation-{id}"),"core_run_id":format!("run-{id}")}})).collect::<Vec<_>>()},
      "inventory":cohort_ids.iter().map(|id|json!({"repository_id":id,"stored_call_rows":"10","effective_endpoint_rows":"10","exact_resolved_rows":"8","admitted_rows":"7","unresolved_placeholder_rows":"2"})).collect::<Vec<_>>(),
      "trails":cohort_ids.iter().map(|id|json!({"repository_id":id,"lengths":[{"length":1,"effective_endpoint":"10","exact_resolved":"8","strictly_admitted":"7"},{"length":2,"effective_endpoint":"9","exact_resolved":"7","strictly_admitted":"6"},{"length":3,"effective_endpoint":"8","exact_resolved":"6","strictly_admitted":"5"},{"length":4,"effective_endpoint":"7","exact_resolved":"5","strictly_admitted":"4"},{"length":5,"effective_endpoint":"6","exact_resolved":"4","strictly_admitted":"3"},{"length":6,"effective_endpoint":"5","exact_resolved":"3","strictly_admitted":"2"}]})).collect::<Vec<_>>(),
      "cases":cases,
      "failure_funnel":{"attempted_positive_steps":312,"classified_positive_steps":312,"unclassified_positive_steps":0,"buckets":[{"outcome":{"kind":"admitted"},"count":"312"}]}
    })
}

fn rebuild_funnel(value: &mut Value) {
    let mut buckets = BTreeMap::<String, (Value, u64)>::new();
    let mut classified = 0u64;
    let mut unclassified = 0u64;
    for case in value["cases"].as_array().expect("cases") {
        unclassified += case["unclassified_step_indices"]
            .as_array()
            .expect("unclassified")
            .len() as u64;
        for step in case["proof_trace"]["steps"].as_array().expect("steps") {
            if step["outcome"]["kind"] == "candidate_limit_exceeded" {
                unclassified += 1;
                continue;
            }
            let outcome: FunnelOutcomeV1 =
                serde_json::from_value(step["outcome"].clone()).expect("closed funnel outcome");
            let key = serde_json::to_string(&outcome).expect("outcome key");
            let entry = buckets
                .entry(key)
                .or_insert((serde_json::to_value(outcome).expect("outcome value"), 0));
            entry.1 += 1;
            classified += 1;
        }
    }
    value["failure_funnel"] = json!({
        "attempted_positive_steps":312,
        "classified_positive_steps":classified,
        "unclassified_positive_steps":unclassified,
        "buckets":buckets.into_values().map(|(outcome, count)|json!({"outcome":outcome,"count":count.to_string()})).collect::<Vec<_>>()
    });
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
    let mut tuned_after_results = thresholds();
    tuned_after_results["automatic"]["minimum_full_proofs"] = json!(95);
    assert!(
        ThresholdsV1::from_json(tuned_after_results).is_err(),
        "in-range threshold tuning must not change the Section 8 freeze"
    );
}

#[test]
fn threshold_identity_is_acyclic_canonical_and_rejects_legacy_corpus_binding() {
    let parsed = ThresholdsV1::from_json(thresholds()).expect("thresholds");
    let baseline = canonical_thresholds_sha256(&parsed).expect("threshold digest");

    let mut legacy = thresholds();
    legacy["corpus_sha256"] = json!(SHA);
    ThresholdsV1::from_json(legacy).expect_err("legacy reverse corpus binding is forbidden");

    let mut semantic_mutation = thresholds();
    semantic_mutation["automatic"]["minimum_full_proofs"] = json!(95);
    let mutated: ThresholdsV1 =
        serde_json::from_value(semantic_mutation).expect("shape-only threshold mutation");
    assert_ne!(
        baseline,
        canonical_thresholds_sha256(&mutated).expect("mutated threshold digest")
    );

    let reordered = serde_json::from_str::<Value>(
        &serde_json::to_string_pretty(&thresholds()).expect("pretty thresholds"),
    )
    .expect("reordered/whitespace JSON");
    let reordered = ThresholdsV1::from_json(reordered).expect("semantic thresholds");
    assert_eq!(
        baseline,
        canonical_thresholds_sha256(&reordered).expect("canonical threshold digest")
    );
}

#[test]
fn canonical_artifact_seam_uses_rfc8785_number_and_utf16_semantics() {
    let numeric = codestory_agent::proof_qualification_support::canonical_json_bytes(
        &json!({"n": 9_007_199_254_740_993u64}),
    )
    .expect("canonical number");
    assert_eq!(
        String::from_utf8(numeric).expect("UTF-8"),
        r#"{"n":9007199254740992}"#
    );

    let mut keys = serde_json::Map::new();
    keys.insert("\u{e000}".into(), json!(2));
    keys.insert("\u{10000}".into(), json!(1));
    let utf16 =
        codestory_agent::proof_qualification_support::canonical_json_bytes(&Value::Object(keys))
            .expect("canonical keys");
    assert_eq!(
        String::from_utf8(utf16).expect("UTF-8"),
        "{\"\u{10000}\":1,\"\u{e000}\":2}"
    );

    let mut rounded_down = corpus();
    rounded_down["paths"][0]["oracle_steps"][0]["callsite"]["file_byte_length"] =
        json!(9_007_199_254_740_992u64);
    let rounded_down = CorpusV1::from_json(rounded_down).expect("safe-integer corpus");
    let mut rounded_from_unsafe = corpus();
    rounded_from_unsafe["paths"][0]["oracle_steps"][0]["callsite"]["file_byte_length"] =
        json!(9_007_199_254_740_993u64);
    let rounded_from_unsafe =
        CorpusV1::from_json(rounded_from_unsafe).expect("unsafe-integer corpus");
    assert_eq!(
        canonical_corpus_sha256(&rounded_down).expect("safe-integer digest"),
        canonical_corpus_sha256(&rounded_from_unsafe).expect("rounded digest"),
        "artifact identity must use the same RFC 8785 number semantics as the sealed seam"
    );
}

#[test]
fn threshold_corpus_summary_digest_dag_rejects_stale_or_mismatched_inputs() {
    let threshold = ThresholdsV1::from_json(thresholds()).expect("thresholds");
    let frozen_corpus = CorpusV1::from_json(corpus()).expect("corpus");
    frozen_corpus
        .validate_against_thresholds(&threshold)
        .expect("one-way threshold binding");
    QualificationSummaryV1::from_json(report())
        .expect("summary")
        .validate_against_inputs(&frozen_corpus, &threshold)
        .expect("summary binds both accepted inputs");

    let mut changed_thresholds = thresholds();
    changed_thresholds["automatic"]["minimum_full_proofs"] = json!(95);
    let changed_thresholds: ThresholdsV1 =
        serde_json::from_value(changed_thresholds).expect("shape-only threshold mutation");
    frozen_corpus
        .validate_against_thresholds(&changed_thresholds)
        .expect_err("changing thresholds invalidates the old corpus binding");

    let old_corpus_hash = canonical_corpus_sha256(&frozen_corpus).expect("old corpus hash");
    let mut rebound = corpus();
    rebound["thresholds_sha256"] =
        json!(canonical_thresholds_sha256(&changed_thresholds).expect("changed threshold hash"));
    let rebound: CorpusV1 = serde_json::from_value(rebound).expect("shape-only rebound corpus");
    assert_ne!(
        old_corpus_hash,
        canonical_corpus_sha256(&rebound).expect("rebound corpus hash")
    );

    let mut wrong_methodology = corpus();
    wrong_methodology["methodology_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    CorpusV1::from_json(wrong_methodology)
        .expect("valid corpus shape")
        .validate_against_thresholds(&threshold)
        .expect_err("methodology must agree");

    let mut wrong_id = corpus();
    wrong_id["corpus_id"] = json!("other-corpus");
    CorpusV1::from_json(wrong_id)
        .expect("valid corpus shape")
        .validate_against_thresholds(&threshold)
        .expect_err("threshold and corpus IDs must agree");

    let mut wrong_count = thresholds();
    wrong_count["expected_positive_steps"] = json!(311);
    let wrong_count: ThresholdsV1 =
        serde_json::from_value(wrong_count).expect("shape-only count mutation");
    frozen_corpus
        .validate_against_thresholds(&wrong_count)
        .expect_err("declared counts must agree");

    let mut altered_corpus = corpus();
    altered_corpus["paths"][0]["source_text"] = json!("altered but valid source text");
    let altered_corpus = CorpusV1::from_json(altered_corpus).expect("altered valid corpus");
    QualificationSummaryV1::from_json(report())
        .expect("summary")
        .validate_against_inputs(&altered_corpus, &threshold)
        .expect_err("summary corpus hash must bind exact corpus semantics");
}

#[test]
fn transport_measurements_preserve_elapsed_ns_and_keep_errors_separate() {
    let measured = TransportEvidenceV1::try_from(Ok(vec![
        codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement {
            revision: "2024-11-05".into(),
            call_tool_result_bytes: vec![1],
            byte_length: 1,
            elapsed_ns: 11,
        },
        codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement {
            revision: "2025-03-26".into(),
            call_tool_result_bytes: vec![2],
            byte_length: 2,
            elapsed_ns: 22,
        },
        codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement {
            revision: "2025-06-18".into(),
            call_tool_result_bytes: vec![3],
            byte_length: 3,
            elapsed_ns: 33,
        },
        codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement {
            revision: "2025-11-25".into(),
            call_tool_result_bytes: vec![4],
            byte_length: 4,
            elapsed_ns: 44,
        },
    ]))
    .expect("transport evidence");
    let value = serde_json::to_value(measured).expect("transport JSON");
    assert_eq!(
        value["measurements"]["measurements"]
            .as_array()
            .expect("measurements")
            .iter()
            .map(|measurement| measurement["elapsed_ns"].as_u64().expect("elapsed"))
            .collect::<Vec<_>>(),
        vec![11, 22, 33, 44]
    );

    let error = TransportEvidenceV1::try_from(Err(
        codestory_cli::proof_qualification_support::ProofQualificationTransportError::OutputSchemaViolation,
    ))
    .expect("typed transport error");
    assert_eq!(
        serde_json::to_value(error).expect("error JSON"),
        json!({"kind":"error","error":{"kind":"output_schema_violation"}})
    );

    let mut independent = report();
    independent["cases"][0]["warm_end_to_end_ms"] = json!(9_999);
    let parsed = QualificationSummaryV1::from_json(independent).expect("independent timings");
    let TransportEvidenceV1::Measurements { measurements } = &parsed.cases[0].transport else {
        panic!("expected measurements")
    };
    assert_eq!(
        measurements
            .measurements
            .iter()
            .map(|measurement| measurement.elapsed_ns)
            .collect::<Vec<_>>(),
        vec![11, 22, 33, 44]
    );

    let mut wrong_order = report();
    wrong_order["cases"][0]["transport"]["measurements"]["measurements"]
        .as_array_mut()
        .expect("measurements")
        .swap(0, 1);
    QualificationSummaryV1::from_json(wrong_order)
        .expect_err("successful transport observations require canonical revision order");
}

#[test]
fn reports_preserve_typed_task_8_to_13_evidence_and_reject_open_gates() {
    let maximal = report();
    let parsed = QualificationSummaryV1::from_json(maximal.clone()).expect("maximal report");
    parsed
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect("report binds all corpus evidence");
    let mut wrong_mutation_binding = report();
    wrong_mutation_binding["cases"][0]["negative_mutations"][0]["target"] = json!("wrong");
    QualificationSummaryV1::from_json(wrong_mutation_binding)
        .expect("summary retains mutation evidence before corpus binding")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("corpus binding rejects altered mutation evidence");
    let mut wrong_corpus_hash = report();
    wrong_corpus_hash["provenance"]["corpus_sha256"] = json!(SHA);
    QualificationSummaryV1::from_json(wrong_corpus_hash)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("provenance binds the supplied corpus bytes");
    let mut wrong_threshold_hash = report();
    wrong_threshold_hash["provenance"]["thresholds_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    QualificationSummaryV1::from_json(wrong_threshold_hash)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("provenance binds the corpus-frozen threshold identity");
    let mut wrong_project = report();
    wrong_project["environment"]["projects"][0]["source_head"] =
        json!("cccccccccccccccccccccccccccccccccccccccc");
    QualificationSummaryV1::from_json(wrong_project)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("project materialization binds cohort source identity");
    let mut wrong_binary = report();
    wrong_binary["environment"]["binary_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    QualificationSummaryV1::from_json(wrong_binary)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("provenance and environment share one binary identity");
    let mut wrong_receipt = report();
    wrong_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]["qualified_name"] =
        json!("wrong");
    QualificationSummaryV1::from_json(wrong_receipt)
        .expect_err("an exact receipt cannot contradict its bound oracle caller");
    let mut lengths = report();
    lengths["trails"][0]["lengths"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(QualificationSummaryV1::from_json(lengths).is_err());
    let mut reason = report();
    reason["cases"][0]["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor","gate":"raw_admission",
        "histogram":[{"reason":{"kind":"raw_admission","reason":"free form"},"edge_ids":[1]}]
    });
    assert!(QualificationSummaryV1::from_json(reason).is_err());
    let mut hard_failure = report();
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

    let mut missing_selector = report();
    missing_selector["cases"][0]["proof_trace"]["selectors"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(QualificationSummaryV1::from_json(missing_selector).is_err());

    let mut non_candidate_edge = report();
    non_candidate_edge["cases"][0]["proof_trace"]["steps"][0]["outcome"]["edge_ids"] = json!([999]);
    assert!(QualificationSummaryV1::from_json(non_candidate_edge).is_err());

    let mut receipt_count_mismatch = report();
    receipt_count_mismatch["cases"][0]["product_disposition"]["authoritative_receipts"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(QualificationSummaryV1::from_json(receipt_count_mismatch).is_err());

    let mut failed_finalization_bytes = report();
    failed_finalization_bytes["cases"][0]["proof_trace"]["finalization"] =
        json!({"kind":"failed","failure":"receipt_budget"});
    assert!(QualificationSummaryV1::from_json(failed_finalization_bytes).is_err());

    let mut invalid_transport = report();
    invalid_transport["cases"][0]["transport"] = json!({
        "kind":"error",
        "error":{"kind":"result_exceeds_budget","maximum_bytes":1,"actual_bytes":65537}
    });
    assert!(QualificationSummaryV1::from_json(invalid_transport).is_err());

    let mut over_projection = report();
    over_projection["cases"][0]["complete_projection_bytes"] = json!(65537);
    assert!(QualificationSummaryV1::from_json(over_projection).is_err());

    let mut missing_measurement = report();
    missing_measurement["cases"][0]["transport"]["measurements"]["measurements"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(QualificationSummaryV1::from_json(missing_measurement).is_err());

    let mut missing_elapsed = report();
    missing_elapsed["cases"][0]["transport"]["measurements"]["measurements"][0]
        .as_object_mut()
        .expect("measurement")
        .remove("elapsed_ns");
    assert!(QualificationSummaryV1::from_json(missing_elapsed).is_err());

    let mut noncanonical_u128 = report();
    noncanonical_u128["inventory"][0]["stored_call_rows"] = json!("01");
    assert!(QualificationSummaryV1::from_json(noncanonical_u128).is_err());

    let mut unknown_variant_field = report();
    unknown_variant_field["cases"][0]["transport"] =
        json!({"kind":"error","error":{"kind":"output_schema_violation","surprise":true}});
    assert!(QualificationSummaryV1::from_json(unknown_variant_field).is_err());

    let duplicate_kind: ActivationDecisionV1 = serde_json::from_value(json!({
        "outcome":"keep_proof_dark","automatic_thresholds_met":false,
        "failed_gates":[
            {"gate_id":"one","kind":"cohort_failure","detail":{"kind":"cohort","repository_id":"rust","observed":"1","required":"30"}},
            {"gate_id":"two","kind":"cohort_failure","detail":{"kind":"cohort","repository_id":"typescript","observed":"1","required":"30"}}
        ]
    }))
    .expect("closed decision DTO");
    duplicate_kind
        .validate()
        .expect("each failed gate has a stable id");

    let delayed_without_evidence: ActivationDecisionV1 = serde_json::from_value(json!({
        "outcome":"delay_full_v3_cut","automatic_thresholds_met":null,"failed_gates":[]
    }))
    .expect("closed decision DTO");
    assert!(delayed_without_evidence.validate().is_err());
    let delayed_with_evidence: ActivationDecisionV1 = serde_json::from_value(json!({
        "outcome":"delay_full_v3_cut",
        "automatic_thresholds_met":null,
        "failed_gates":[{"gate_id":"packet-proof-dependency","kind":"integration_dependency","detail":{"kind":"source_dependency","evidence":{"source_path":"src/lib.rs","source_range":range(0,10),"source_sha256":SHA,"dependency":"v3_packet_requires_proof","passing_test":{"test_id":"packet_v3_requires_proof","kind":"packet_v3_requires_proof","status":"passed"}}}}]
    }))
    .expect("closed decision DTO");
    delayed_with_evidence
        .validate()
        .expect("outcome D uses closed source evidence");
    let wrong_dependency_gate: ActivationDecisionV1 = serde_json::from_value(json!({
        "outcome":"keep_proof_dark","automatic_thresholds_met":false,
        "failed_gates":[{"gate_id":"wrong","kind":"response_size","detail":{
            "kind":"source_dependency",
            "evidence":{"source_path":"src/lib.rs","source_range":range(0,10),"source_sha256":SHA,"dependency":"v3_packet_requires_proof","passing_test":{"test_id":"packet_v3_requires_proof","kind":"packet_v3_requires_proof","status":"passed"}}
        }}]
    }))
    .expect("closed decision DTO");
    assert!(wrong_dependency_gate.validate().is_err());

    let transport_gate: ActivationDecisionV1 = serde_json::from_value(json!({
        "outcome":"keep_proof_dark","automatic_thresholds_met":false,
        "failed_gates":[{
            "gate_id":"transport-over-cap","kind":"response_size",
            "detail":{"kind":"transport","evidence":{"kind":"error","error":{"kind":"result_exceeds_budget","maximum_bytes":65536,"actual_bytes":65537}}}
        }]
    }))
    .expect("closed decision DTO");
    transport_gate
        .validate()
        .expect("transport gate evidence is closed and representable");
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

    for failure in ["receipt_integration", "receipt_budget", "projection_budget"] {
        let mut value = report();
        value["cases"][0]["proof_trace"]["finalization"] =
            json!({"kind":"failed","failure":failure});
        value["cases"][0]["complete_projection_bytes"] = json!(0);
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

#[test]
fn producer_facade_conversions_preserve_task4_and_task6_semantics() {
    use codestory_agent::proof_qualification_support::{
        CallableContainmentEvidence, IndexedCallEdgeReceipt, IndexedLineWindow, PinnedNodeIdentity,
        ReceiptRef, ResolvedNodeIdentity, UnavailableReason,
    };
    use codestory_contracts::graph::{NodeId, ResolutionCertainty};
    use codestory_runtime::proof_qualification_support::{
        CandidateFailure, CandidateGate, ContainmentFailure, FinalizationFailure,
        FinalizationTrace, ProofQualificationTrace, SelectorFailure, SelectorGateOutcome,
        SelectorQualificationTrace, SourceBindingFailure,
    };

    let trace = ProofQualificationTrace {
        selectors: vec![
            SelectorQualificationTrace {
                selector_index: 0,
                outcome: SelectorGateOutcome::Failed(SelectorFailure::Missing),
            },
            SelectorQualificationTrace {
                selector_index: 1,
                outcome: SelectorGateOutcome::Resolved {
                    node_id: NodeId(-7),
                },
            },
            SelectorQualificationTrace {
                selector_index: 2,
                outcome: SelectorGateOutcome::Unavailable(UnavailableReason::ProofFactsUnavailable),
            },
        ],
        selector_early_return: true,
        steps: vec![],
        finalization: FinalizationTrace::Complete {
            projection_bytes: 512,
        },
    };
    let converted = ProofQualificationTraceV1::try_from(trace).expect("convert trace");
    assert_eq!(converted.selectors.len(), 3, "all selectors are retained");
    assert!(
        converted.selector_early_return,
        "a failure before the last selector returns early"
    );
    assert_eq!(
        serde_json::to_value(&converted.selectors[1]).unwrap()["outcome"]["node_id"],
        -7
    );

    for failure in [
        FinalizationFailure::ReceiptIntegration,
        FinalizationFailure::ReceiptBudget,
        FinalizationFailure::ProjectionBudget,
    ] {
        assert!(matches!(
            FinalizationTraceV1::try_from(FinalizationTrace::Failed(failure)).unwrap(),
            FinalizationTraceV1::Failed { .. }
        ));
    }

    for gate in [
        CandidateGate::RawAdmission,
        CandidateGate::Containment,
        CandidateGate::SourceBinding,
        CandidateGate::Line,
    ] {
        let _: CandidateGateV1 = gate.into();
    }
    for failure in [
        SelectorFailure::Missing,
        SelectorFailure::Ambiguous,
        SelectorFailure::NonCallable,
    ] {
        let _: SelectorGateOutcomeV1 = SelectorGateOutcome::Failed(failure).into();
    }
    for unavailable in [
        UnavailableReason::ValidatedContractHashMismatch,
        UnavailableReason::PublicationPinMismatch,
        UnavailableReason::SourceNotBoundToPublication,
        UnavailableReason::ProofFactsUnavailable,
    ] {
        let _: SelectorGateOutcomeV1 = SelectorGateOutcome::Unavailable(unavailable).into();
    }
    for reason in [
        codestory_agent::proof_qualification_support::RawAdmissionFailure::WrongKind,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CertaintyAbsent,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CertaintyProbable,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CertaintyUncertain,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::WrongEffectiveSource,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::WrongEffectiveTarget,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::MissingExactResolvedTarget,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CandidateAlternativesRetained,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::MissingFileNode,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::MissingLine,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CallsiteFileMismatch,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CallsiteLineMismatch,
        codestory_agent::proof_qualification_support::RawAdmissionFailure::CallsiteRawTargetMismatch,
    ] {
        let _: CandidateFailureV1 = CandidateFailure::RawAdmission(reason).into();
    }
    for reason in [
        ContainmentFailure::EdgeSourceFileMismatch,
        ContainmentFailure::Missing,
        ContainmentFailure::Ambiguous,
    ] {
        let _: CandidateFailureV1 = CandidateFailure::Containment(reason).into();
    }
    for reason in [
        SourceBindingFailure::FileIncomplete,
        SourceBindingFailure::StoredHashAbsent,
        SourceBindingFailure::WorkingTreeReadFailed,
        SourceBindingFailure::WorkingTreeHashMismatch,
        SourceBindingFailure::InvalidUtf8,
        SourceBindingFailure::LineMissing,
        SourceBindingFailure::LineOverLimit,
    ] {
        let _: CandidateFailureV1 = CandidateFailure::SourceBinding(reason).into();
    }

    for error in [
        codestory_cli::proof_qualification_support::ProofQualificationTransportError::Serialization("encode".into()),
        codestory_cli::proof_qualification_support::ProofQualificationTransportError::InvalidProjection("root".into()),
        codestory_cli::proof_qualification_support::ProofQualificationTransportError::OutputSchemaViolation,
        codestory_cli::proof_qualification_support::ProofQualificationTransportError::ResultExceedsBudget { maximum_bytes: 65_536, actual_bytes: 65_537 },
    ] {
        assert!(matches!(
            TransportEvidenceV1::try_from(Err::<Vec<codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement>, _>(error)).unwrap(),
            TransportEvidenceV1::Error { .. }
        ));
    }
    let measurements = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
        .into_iter()
        .map(|revision| {
            codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement {
                revision: revision.to_owned(),
                call_tool_result_bytes: vec![0; 128],
                byte_length: 128,
                elapsed_ns: 1,
            }
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        TransportEvidenceV1::try_from(Ok::<
            _,
            codestory_cli::proof_qualification_support::ProofQualificationTransportError,
        >(measurements))
        .unwrap(),
        TransportEvidenceV1::Measurements { .. }
    ));

    let identity = |node_id: &str, qualified_name: &str| ResolvedNodeIdentity {
        pinned: PinnedNodeIdentity {
            project_id: "project".into(),
            core_generation_id: "generation".into(),
            core_run_id: "run".into(),
            node_id: node_id.into(),
        },
        canonical_id: format!("canonical-{node_id}"),
        qualified_name: qualified_name.into(),
        project_file_components: vec!["src".into(), "lib.rs".into()],
    };
    let task6_receipt = IndexedCallEdgeReceipt {
        receipt: ReceiptRef {
            receipt_id: "indexed-call-edge:fixture".into(),
            edge_id: "-42".into(),
        },
        source: identity("-1", "crate::start"),
        target: identity("-2", "crate::target_0"),
        certainty: ResolutionCertainty::Certain,
        callsite_identity: "-3:1:0:-2|fixture".into(),
        containment: CallableContainmentEvidence {
            file_node_id: NodeId(-3),
            owner_node_id: NodeId(-1),
            start_line: 1,
            end_line: 1,
        },
        line_window: IndexedLineWindow {
            kind: "indexed_line_v1",
            project_file_components: vec!["src".into(), "lib.rs".into()],
            indexed_sha256: SHA.into(),
            observed_sha256: SHA.into(),
            anchor_line: 1,
            byte_start: 11,
            byte_end: 19,
            text: "call();\n".into(),
        },
    };
    let comparison: ReceiptOracleComparisonV1 = serde_json::from_value(
        report()["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]
            .clone(),
    )
    .expect("oracle comparison");
    let observed = ObservedReceiptV1::from_task6(0, &task6_receipt, comparison)
        .expect("lossless Task 6 receipt conversion");
    assert_eq!(observed.receipt_id, "indexed-call-edge:fixture");
    assert_eq!(observed.edge_id, -42);
    assert_eq!(observed.source.pinned.project_id, "project");
    assert_eq!(observed.source.pinned.core_generation_id, "generation");
    assert_eq!(observed.source.pinned.core_run_id, "run");
    assert_eq!(observed.source.pinned.node_id, "-1");
    assert_eq!(observed.source.canonical_id, "canonical--1");
    assert_eq!(observed.source.qualified_name, "crate::start");
    assert_eq!(observed.source.project_file_components, ["src", "lib.rs"]);
    assert_eq!(observed.callsite_line, 1);
    assert_eq!(observed.callsite_identity, "-3:1:0:-2|fixture");
    assert_eq!(observed.certainty, contracts::ReceiptCertaintyV1::Certain);
    assert_eq!(observed.containment.file_node_id, -3);
    assert_eq!(observed.containment.owner_node_id, -1);
    assert_eq!(observed.containment.start_line, 1);
    assert_eq!(observed.containment.end_line, 1);
    assert_eq!(observed.line_window.byte_start, 11);
    assert_eq!(observed.target.pinned.project_id, "project");
    assert_eq!(observed.target.pinned.core_generation_id, "generation");
    assert_eq!(observed.target.pinned.core_run_id, "run");
    assert_eq!(observed.target.pinned.node_id, "-2");
    assert_eq!(observed.target.canonical_id, "canonical--2");
    assert_eq!(observed.target.qualified_name, "crate::target_0");
    assert_eq!(observed.target.project_file_components, ["src", "lib.rs"]);

    let mut probable = task6_receipt;
    probable.certainty = ResolutionCertainty::Probable;
    let comparison: ReceiptOracleComparisonV1 = serde_json::from_value(
        report()["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]
            .clone(),
    )
    .expect("oracle comparison");
    ObservedReceiptV1::from_task6(0, &probable, comparison)
        .expect_err("Task 6 receipts must retain and require Certain certainty");
}

fn assert_valid_first_zero(gate: &str, kind: &str, reason: &str) {
    let mut value = report();
    let case = &mut value["cases"][0];
    let observed = case["receipt_evidence"]["observed_receipts"]
        .as_array_mut()
        .expect("observed receipts")
        .remove(0);
    case["product_disposition"] = json!({
        "kind":"unknown","gaps":["relation_missing"],"authoritative_receipts":[]
    });
    case["actionable_exact_gap"] = json!("relation_missing");
    case["receipt_evidence"]["missing_oracle_steps"] = json!([{
        "step_index":0,"oracle_step":observed["oracle_comparison"]["oracle_step"]
    }]);
    case["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor",
        "gate":gate,
        "histogram":[{"reason":{"kind":kind,"reason":reason},"edge_ids":[1]}]
    });
    rebuild_funnel(&mut value);
    QualificationSummaryV1::from_json(value).expect("mapped first-zero outcome");
}

fn make_non_proven_case(value: &mut Value, disposition: &str, gap: Option<&str>) {
    let case = &mut value["cases"][0];
    let missing = case["receipt_evidence"]["observed_receipts"]
        .as_array()
        .expect("observed receipts")
        .iter()
        .map(|receipt| {
            json!({
                "step_index":receipt["step_index"],
                "oracle_step":receipt["oracle_comparison"]["oracle_step"]
            })
        })
        .collect::<Vec<_>>();
    case["receipt_evidence"]["missing_oracle_steps"] = Value::Array(missing);
    case["product_disposition"] = json!({
        "kind":disposition,
        "gaps":gap.into_iter().collect::<Vec<_>>(),
        "authoritative_receipts":[]
    });
    case["actionable_exact_gap"] = gap.map(Value::from).unwrap_or(Value::Null);
}

#[test]
fn invariant_table_exercises_remaining_closed_decision_and_funnel_variants() {
    for disposition in ["unknown", "certified_absence", "invalid"] {
        let mut value = report();
        make_non_proven_case(
            &mut value,
            disposition,
            (disposition == "unknown").then_some("selector_missing"),
        );
        QualificationSummaryV1::from_json(value).expect("closed product disposition");
    }
    let mut mismatched_receipt = report();
    make_non_proven_case(&mut mismatched_receipt, "invalid", None);
    mismatched_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0]["target"]["qualified_name"] =
        json!("crate::false_positive");
    let oracle = mismatched_receipt["cases"][0]["receipt_evidence"]["observed_receipts"]
        [0]["oracle_comparison"]["oracle_step"]
        .clone();
    mismatched_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"] = json!({
        "kind":"mismatched","oracle_step_index":0,"oracle_step":oracle,
        "mismatches":["target"]
    });
    QualificationSummaryV1::from_json(mismatched_receipt)
        .expect("closed mismatched receipt comparison");
    for gap in [
        "selector_missing",
        "selector_ambiguous",
        "relation_missing",
        "recursion",
        "source_binding",
        "projection_budget",
    ] {
        let mut value = report();
        make_non_proven_case(&mut value, "unknown", Some(gap));
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
        let value: ActivationDecisionV1 = serde_json::from_value(json!({
            "outcome":"keep_proof_dark","automatic_thresholds_met":false,
            "failed_gates":[{"gate_id":format!("{gate}-gate"),"kind":gate,
                "detail":{"kind":"count","observed":"1","required":"1"}}]
        }))
        .expect("closed qualification gate");
        value.validate().expect("valid count gate detail");
    }
    for outcome in [
        "public_exact_verifier",
        "experimental_manual_verifier",
        "keep_proof_dark",
    ] {
        let value: ActivationDecisionV1 = serde_json::from_value(json!({
            "outcome":outcome,"automatic_thresholds_met":false,"failed_gates":[]
        }))
        .expect("closed activation outcome");
        value.validate().expect("non-delay outcome");
    }
    let transport_dependency: ActivationDecisionV1 = serde_json::from_value(json!({
        "outcome":"delay_full_v3_cut",
        "automatic_thresholds_met":null,
        "failed_gates":[{"gate_id":"transport-keep-dark-dependency","kind":"integration_dependency","detail":{"kind":"source_dependency","evidence":{"source_path":"src/lib.rs","source_range":range(0,10),"source_sha256":SHA,"dependency":"transport_cannot_represent_keep_dark","passing_test":{"test_id":"transport_cannot_represent_keep_dark","kind":"transport_cannot_represent_keep_dark","status":"passed"}}}}]
    }))
    .expect("second source dependency variant");
    transport_dependency
        .validate()
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
    assert!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["TransportMeasurementV1"]
            ["properties"]
            .get("elapsed_ns")
            .is_some()
    );
    assert!(
        contracts::schema_json(SchemaDocument::Thresholds)["properties"]
            .get("corpus_sha256")
            .is_none()
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ProductDispositionV1"]["properties"]
            ["authoritative_receipts"]["maxItems"],
        6
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ObservedReceiptV1"]["properties"]
            ["step_index"]["maximum"],
        5
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["MissingOracleStepV1"]["properties"]
            ["step_index"]["maximum"],
        5
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ObservedReceiptV1"]["properties"]
            ["receipt_id"]["minLength"],
        1
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ReceiptEvidenceV1"]["properties"]
            ["missing_oracle_steps"]["maxItems"],
        6
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ReceiptEvidenceV1"]["properties"]
            ["observed_receipts"]["maxItems"],
        MAX_OBSERVED_RECEIPTS_PER_CASE
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["StepQualificationTraceV1"]["properties"]
            ["candidate_edge_ids"]["maxItems"],
        MAX_CANDIDATE_EDGES_PER_STEP
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["StepQualificationOutcomeV1"]["oneOf"]
            [2]["properties"]["maximum_candidate_edges"]["const"],
        MAX_CANDIDATE_EDGES_PER_STEP
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["StepQualificationOutcomeV1"]["oneOf"]
            [2]["properties"]["observed_candidate_edges_at_least"]["const"],
        MAX_CANDIDATE_EDGES_PER_STEP + 1
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ObservedLineWindowV1"]["properties"]
            ["kind"]["const"],
        "indexed_line_v1"
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ObservedLineWindowV1"]["properties"]
            ["text"]["maxLength"],
        8192
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ReceiptOracleComparisonV1"]["oneOf"]
            [1]["properties"]["mismatches"]["maxItems"],
        4
    );
    assert!(
        contracts::schema_json(SchemaDocument::Report)["properties"]
            .get("decision")
            .is_none()
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
            "--corpus",
            "/tmp/c",
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
        cli::Cli::try_parse_from([
            "bin",
            "verify",
            "--thresholds",
            "/tmp/t",
            "--results",
            "/tmp/r"
        ])
        .is_err(),
        "verify must receive the corpus whose identity it validates"
    );
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

#[test]
fn producer_receipts_allow_multiple_rows_per_step_and_derive_metrics() {
    let mut value = report();
    add_extra_observed_receipt(&mut value, false);

    let parsed = QualificationSummaryV1::from_json(value)
        .expect("Task 6 may admit multiple receipt-bearing edges for one step");
    parsed
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("corpus"))
        .expect("both rows bind the same frozen oracle step");
    let case = parsed.cases[0]
        .receipt_metrics()
        .expect("derived case metrics");
    assert_eq!(case.observed_receipt_count, 2);
    assert_eq!(case.authoritative_receipt_count, 1);
    assert_eq!(case.proven_step_precision_milli, 1000);
    assert_eq!(case.proven_step_recall_milli, 1000);
    let aggregate = parsed.receipt_metrics().expect("derived aggregate metrics");
    assert_eq!(aggregate.observed_receipt_count, 313);
    assert_eq!(aggregate.authoritative_receipt_count, 312);
    assert_eq!(aggregate.positive_step_precision_milli, 1000);
    assert_eq!(aggregate.positive_step_recall_milli, 1000);
}

#[test]
fn false_positive_extra_edges_remain_observed_without_becoming_authoritative() {
    let mut value = report();
    add_extra_observed_receipt(&mut value, true);

    let mut falsely_authoritative = value.clone();
    let extra =
        falsely_authoritative["cases"][0]["receipt_evidence"]["observed_receipts"][1].clone();
    falsely_authoritative["cases"][0]["product_disposition"]["authoritative_receipts"]
        .as_array_mut()
        .expect("authoritative receipts")
        .push(json!({"receipt_id":extra["receipt_id"],"edge_id":extra["edge_id"]}));
    let falsely_authoritative = QualificationSummaryV1::from_json(falsely_authoritative)
        .expect("wrong sealed product outcomes remain reportable evidence");
    let facts = falsely_authoritative.cases[0]
        .evaluable_facts()
        .expect("derived evaluable facts");
    assert!(facts.false_contract_proven);
    assert!(!facts.product_disposition_matches_evidence);

    let parsed = QualificationSummaryV1::from_json(value)
        .expect("an honest false-positive extra remains concrete diagnostic evidence");
    parsed
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("corpus"))
        .expect("the mismatched comparison binds the exact frozen oracle row");
    let metrics = parsed.cases[0].receipt_metrics().expect("derived metrics");
    assert_eq!(metrics.observed_receipt_count, 2);
    assert_eq!(metrics.false_positive_receipt_count, 1);
    assert_eq!(metrics.authoritative_receipt_count, 1);
    assert!(metrics.all_authoritative_receipts_exact);
}

#[test]
fn wrong_contract_proven_with_missing_authoritative_evidence_remains_evaluable() {
    let mut value = report();
    let case = &mut value["cases"][0];
    let reference = case["product_disposition"]["authoritative_receipts"]
        .as_array_mut()
        .expect("authoritative receipts")
        .remove(0);
    let receipt = case["receipt_evidence"]["observed_receipts"]
        .as_array()
        .expect("observed receipts")
        .iter()
        .find(|receipt| receipt["receipt_id"] == reference["receipt_id"])
        .expect("referenced receipt")
        .clone();
    case["receipt_evidence"]["missing_oracle_steps"] = json!([{
        "step_index":receipt["step_index"],
        "oracle_step":receipt["oracle_comparison"]["oracle_step"]
    }]);

    let parsed = QualificationSummaryV1::from_json(value)
        .expect("a false sealed ContractProven result is evidence, not a DTO error");
    let facts = parsed.cases[0]
        .evaluable_facts()
        .expect("derived evaluable facts");
    assert!(facts.false_contract_proven);
    assert!(!facts.product_disposition_matches_evidence);
    assert!(!facts.contract_proven_supported);
}

#[test]
fn receipt_provenance_is_bound_to_environment_selectors_and_containment() {
    let mutations = [
        (
            "generation",
            vec!["source", "pinned", "core_generation_id"],
            json!("wrong-generation"),
        ),
        (
            "run",
            vec!["target", "pinned", "core_run_id"],
            json!("wrong-run"),
        ),
        (
            "node",
            vec!["source", "pinned", "node_id"],
            json!("-999999"),
        ),
        (
            "containment",
            vec!["containment", "owner_node_id"],
            json!(-999999),
        ),
        ("certainty", vec!["certainty"], json!("probable")),
    ];
    for (label, path, replacement) in mutations {
        let mut value = report();
        let mut slot = &mut value["cases"][0]["receipt_evidence"]["observed_receipts"][0];
        for component in path {
            slot = &mut slot[component];
        }
        *slot = replacement;
        assert!(
            QualificationSummaryV1::from_json(value).is_err(),
            "{label} provenance mutation must be rejected"
        );
    }
}

#[test]
fn receipts_unrelated_to_task6_admitted_edges_are_rejected() {
    let mut value = report();
    value["cases"][0]["receipt_evidence"]["observed_receipts"][0]["edge_id"] = json!(999_999);
    value["cases"][0]["product_disposition"]["authoritative_receipts"][0]["edge_id"] =
        json!(999_999);

    QualificationSummaryV1::from_json(value)
        .expect_err("every observed receipt edge must come from the matching admitted step");

    let mut dangling = report();
    dangling["cases"][0]["product_disposition"]["authoritative_receipts"][0]["receipt_id"] =
        json!("indexed-call-edge:not-observed");
    QualificationSummaryV1::from_json(dangling)
        .expect_err("authoritative receipt references must resolve to concrete observed rows");
}

#[test]
fn exact_oracle_claims_reject_mismatched_line_window_and_target() {
    let mut line = report();
    line["cases"][0]["receipt_evidence"]["observed_receipts"][0]["callsite_line"] = json!(999);
    line["cases"][0]["receipt_evidence"]["observed_receipts"][0]["callsite_identity"] =
        json!("-10000:999:0:-2|fixture");
    line["cases"][0]["receipt_evidence"]["observed_receipts"][0]["containment"]["end_line"] =
        json!(999);
    QualificationSummaryV1::from_json(line)
        .expect_err("an exact oracle claim cannot carry a mismatched callsite line");

    let mut window = report();
    window["cases"][0]["receipt_evidence"]["observed_receipts"][0]["line_window"]["byte_start"] =
        json!(12);
    window["cases"][0]["receipt_evidence"]["observed_receipts"][0]["line_window"]["text"] =
        json!("all();\n");
    QualificationSummaryV1::from_json(window)
        .expect_err("an exact oracle claim cannot carry a mismatched byte window");

    let mut target = report();
    target["cases"][0]["receipt_evidence"]["observed_receipts"][0]["target"]["qualified_name"] =
        json!("crate::wrong_target");
    QualificationSummaryV1::from_json(target)
        .expect_err("an exact oracle claim cannot carry a mismatched target");
}

#[test]
fn missing_oracle_steps_are_separate_exact_rows() {
    let mut value = report();
    let case = &mut value["cases"][0];
    let observed = case["receipt_evidence"]["observed_receipts"]
        .as_array_mut()
        .expect("observed receipts")
        .remove(0);
    case["product_disposition"]["authoritative_receipts"]
        .as_array_mut()
        .expect("authoritative receipts")
        .remove(0);
    case["receipt_evidence"]["missing_oracle_steps"] = json!([{
        "step_index":0,
        "oracle_step":observed["oracle_comparison"]["oracle_step"]
    }]);
    case["product_disposition"] = json!({
        "kind":"unknown","gaps":["relation_missing"],"authoritative_receipts":[]
    });
    case["actionable_exact_gap"] = json!("relation_missing");
    case["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor","gate":"raw_admission",
        "histogram":[{"reason":{"kind":"raw_admission","reason":"wrong_kind"},"edge_ids":[1]}]
    });
    rebuild_funnel(&mut value);

    let mut omitted = value.clone();
    omitted["cases"][0]["receipt_evidence"]["missing_oracle_steps"] = json!([]);
    QualificationSummaryV1::from_json(omitted)
        .expect_err("an uncovered oracle step requires separate missing evidence");
    let mut wrong_oracle = value.clone();
    wrong_oracle["cases"][0]["receipt_evidence"]["missing_oracle_steps"][0]["oracle_step"]["target"]
        ["symbol"] = json!("crate::wrong_target");
    QualificationSummaryV1::from_json(wrong_oracle)
        .expect("missing row is structurally closed")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("corpus"))
        .expect_err("missing row must carry the exact frozen oracle data");

    let parsed = QualificationSummaryV1::from_json(value).expect("separate missing oracle row");
    parsed
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("corpus"))
        .expect("missing row carries the exact frozen oracle step");
    let metrics = parsed.cases[0].receipt_metrics().expect("derived metrics");
    assert_eq!(metrics.observed_receipt_count, 0);
    assert_eq!(metrics.missing_oracle_step_count, 1);
    assert_eq!(metrics.proven_step_recall_milli, 0);
}

#[test]
fn caller_supplied_metrics_and_precomputed_activation_outcomes_are_rejected() {
    let mut empty_receipt = report();
    empty_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0]["receipt_id"] = json!("");
    empty_receipt["cases"][0]["product_disposition"]["authoritative_receipts"][0]["receipt_id"] =
        json!("");
    QualificationSummaryV1::from_json(empty_receipt)
        .expect_err("receipt IDs are producer identities, never empty strings");

    for field in [
        "authoritative_receipt_count",
        "oracle_receipts_exact",
        "proven_step_precision_milli",
        "proven_step_recall_milli",
        "proven_prefix_length",
        "diagnostic_candidate_count",
        "authoritative_receipt_evidence_count",
    ] {
        let mut value = report();
        value["cases"][0][field] = json!(1000);
        QualificationSummaryV1::from_json(value)
            .expect_err("derived metrics are not accepted report inputs");
    }

    let mut value = report();
    value["decision"] = json!({
        "outcome": "public_exact_verifier",
        "failed_gates": [{
            "gate_id": "hard-receipt-mismatch",
            "kind": "receipt_mismatch",
            "detail": {"kind": "count", "observed": "1", "required": "0"}
        }],
        "automatic_thresholds_met": true
    });

    QualificationSummaryV1::from_json(value)
        .expect_err("Task 9 owns activation decisions; Task 7 must not accept a contradiction");
}

#[test]
fn all_materialization_freshness_variants_are_closed_and_representable() {
    for freshness in ["fresh", "stale", "missing"] {
        let mut value = report();
        value["environment"]["projects"][0]["freshness"] = json!(freshness);
        QualificationSummaryV1::from_json(value).expect("closed freshness variant");
    }
    let mut unknown = report();
    unknown["environment"]["projects"][0]["freshness"] = json!("unknown");
    QualificationSummaryV1::from_json(unknown).expect_err("freshness is a closed enum");
}

#[test]
fn candidate_and_observed_receipt_bounds_cover_exact_cap_and_cap_plus_one() {
    let mut exact = report();
    expand_six_step_case_to_candidate_cap(&mut exact, 28);
    let parsed = QualificationSummaryV1::from_json(exact.clone())
        .expect("the exact producer cap remains a complete classified result");
    assert_eq!(
        parsed.cases[28]
            .receipt_metrics()
            .unwrap()
            .observed_receipt_count,
        u64::try_from(MAX_OBSERVED_RECEIPTS_PER_CASE).unwrap()
    );

    let rows: Vec<ObservedReceiptV1> =
        serde_json::from_value(exact["cases"][28]["receipt_evidence"]["observed_receipts"].clone())
            .expect("bounded receipt rows");
    assert!(matches!(
        contracts::ReceiptEvidenceV1::bounded(rows.clone(), vec![]),
        contracts::ReceiptEvidenceBuildOutcomeV1::Complete(_)
    ));
    let mut over_rows = rows;
    over_rows.push(over_rows[0].clone());
    assert!(matches!(
        contracts::ReceiptEvidenceV1::bounded(over_rows, vec![]),
        contracts::ReceiptEvidenceBuildOutcomeV1::LimitExceeded {
            maximum_observed_receipts,
            observed_receipts_at_least,
        } if maximum_observed_receipts == MAX_OBSERVED_RECEIPTS_PER_CASE
            && observed_receipts_at_least == MAX_OBSERVED_RECEIPTS_PER_CASE + 1
    ));

    let mut truncated = report();
    let case = &mut truncated["cases"][0];
    let oracle_step =
        case["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]["oracle_step"]
            .clone();
    case["receipt_evidence"]["observed_receipts"] = json!([]);
    case["receipt_evidence"]["missing_oracle_steps"] =
        json!([{"step_index":0,"oracle_step":oracle_step}]);
    case["product_disposition"]["authoritative_receipts"] = json!([]);
    case["proof_trace"]["steps"][0]["candidate_edge_ids"] = Value::Array(
        (1..=MAX_CANDIDATE_EDGES_PER_STEP)
            .map(|edge| json!(edge))
            .collect(),
    );
    case["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"candidate_limit_exceeded",
        "maximum_candidate_edges":MAX_CANDIDATE_EDGES_PER_STEP,
        "observed_candidate_edges_at_least":MAX_CANDIDATE_EDGES_PER_STEP + 1
    });
    rebuild_funnel(&mut truncated);
    let parsed = QualificationSummaryV1::from_json(truncated)
        .expect("cap + 1 becomes a typed unclassified observation, never a partial proof");
    assert_eq!(parsed.failure_funnel.classified_positive_steps, 311);
    assert_eq!(parsed.failure_funnel.unclassified_positive_steps, 1);
    assert!(
        parsed.cases[0]
            .evaluable_facts()
            .unwrap()
            .false_contract_proven
    );
}

fn expand_six_step_case_to_candidate_cap(value: &mut Value, case_index: usize) {
    let case = &mut value["cases"][case_index];
    assert_eq!(case["attempted_step_count"], 6);
    let templates = case["receipt_evidence"]["observed_receipts"]
        .as_array()
        .expect("observed receipts")
        .clone();
    let mut observed_receipts = Vec::with_capacity(MAX_OBSERVED_RECEIPTS_PER_CASE);
    let mut authoritative_receipts = Vec::with_capacity(6);
    for step_index in 0..6usize {
        let base = 2_000_000i64
            + i64::try_from(step_index * MAX_CANDIDATE_EDGES_PER_STEP).expect("edge base");
        let edge_ids = (0..MAX_CANDIDATE_EDGES_PER_STEP)
            .map(|offset| base + i64::try_from(offset).expect("edge offset"))
            .collect::<Vec<_>>();
        case["proof_trace"]["steps"][step_index]["candidate_edge_ids"] =
            serde_json::to_value(&edge_ids).expect("candidate ids");
        case["proof_trace"]["steps"][step_index]["outcome"]["edge_ids"] =
            serde_json::to_value(&edge_ids).expect("admitted ids");
        for (offset, edge_id) in edge_ids.into_iter().enumerate() {
            let mut receipt = templates[step_index].clone();
            receipt["receipt_id"] = json!(format!(
                "indexed-call-edge:cap-{case_index}-{step_index}-{offset}"
            ));
            receipt["edge_id"] = json!(edge_id);
            if offset == 0 {
                authoritative_receipts.push(json!({
                    "receipt_id":receipt["receipt_id"],
                    "edge_id":edge_id
                }));
            }
            observed_receipts.push(receipt);
        }
    }
    case["receipt_evidence"]["observed_receipts"] = Value::Array(observed_receipts);
    case["product_disposition"]["authoritative_receipts"] = Value::Array(authoritative_receipts);
}

fn add_extra_observed_receipt(value: &mut Value, mismatched: bool) {
    let case = &mut value["cases"][0];
    let step = &mut case["proof_trace"]["steps"][0];
    let original_edge = step["candidate_edge_ids"][0]
        .as_i64()
        .expect("fixture edge id");
    let extra_edge = original_edge + 1_000_000;
    step["candidate_edge_ids"]
        .as_array_mut()
        .expect("candidate ids")
        .push(json!(extra_edge));
    step["outcome"]["edge_ids"]
        .as_array_mut()
        .expect("admitted ids")
        .push(json!(extra_edge));
    let mut extra = case["receipt_evidence"]["observed_receipts"][0].clone();
    let file_node_id = extra["containment"]["file_node_id"]
        .as_i64()
        .expect("file node id");
    let target_node_id = extra["target"]["pinned"]["node_id"]
        .as_str()
        .expect("target node id")
        .to_owned();
    extra["receipt_id"] = json!(format!("indexed-call-edge:extra-{extra_edge}"));
    extra["edge_id"] = json!(extra_edge);
    extra["callsite_identity"] = json!(format!("{file_node_id}:1:0:{target_node_id}|extra"));
    if mismatched {
        extra["callsite_line"] = json!(99);
        extra["callsite_identity"] = json!(format!("{file_node_id}:99:0:{target_node_id}|extra"));
        extra["containment"]["end_line"] = json!(99);
        extra["line_window"]["byte_start"] = json!(12);
        extra["line_window"]["byte_end"] = json!(19);
        extra["line_window"]["text"] = json!("all();\n");
        let oracle = extra["oracle_comparison"]["oracle_step"].clone();
        extra["oracle_comparison"] = json!({
            "kind":"mismatched","oracle_step_index":0,"oracle_step":oracle,
            "mismatches":["callsite_line","callsite_window"]
        });
    }
    case["receipt_evidence"]["observed_receipts"]
        .as_array_mut()
        .expect("observed receipts")
        .push(extra);
}
