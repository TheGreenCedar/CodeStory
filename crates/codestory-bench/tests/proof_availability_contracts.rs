#[path = "../src/bin/codestory_proof_availability/cli.rs"]
mod cli;
#[allow(dead_code)] // This test includes only the contract surface, not report construction.
#[path = "../src/bin/codestory_proof_availability/contracts.rs"]
mod contracts;

use clap::Parser;
use contracts::{
    ActivationDecisionV1, ActualProductResultV1, CandidateFailureV1, CandidateGateV1,
    CaseValidationFailure, ClauseClassificationV1, CohortPathFileV1, CorpusV1,
    ExactScopeSelectorV1, ExactSymbolSelectorV1, FinalizationTraceV1, FunnelOutcomeV1,
    MAX_CANDIDATE_EDGES_PER_STEP, MAX_OBSERVED_RECEIPTS_PER_CASE, ObservedReceiptV1,
    ProofContractFieldV1, ProofQualificationTraceV1, QualificationSummaryV1,
    ReceiptOracleComparisonV1, SchemaDocument, SelectorGateOutcomeV1, ThresholdsV1,
    TransportEvidenceV1, canonical_cohort_path_file_sha256, canonical_corpus_sha256,
    canonical_thresholds_sha256,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn rebind_results_digest(value: &mut Value) {
    value["provenance"]["results_sha256"] =
        json!(contracts::results_evidence_sha256_from_json(value).unwrap());
}

#[test]
fn task11a_results_digest_is_non_circular_order_stable_and_evidence_complete() {
    let value = report();
    let parsed = QualificationSummaryV1::from_json(value.clone()).expect("report fixture");
    let expected = contracts::results_evidence_sha256(
        &parsed.environment,
        &parsed.inventory,
        &parsed.trails,
        &parsed.cases,
        &parsed.failure_funnel,
    )
    .expect("results evidence digest");

    let mut reordered = parsed.clone();
    reordered.environment.projects.reverse();
    reordered.inventory.reverse();
    reordered.trails.reverse();
    reordered.cases.reverse();
    assert_eq!(
        contracts::results_evidence_sha256(
            &reordered.environment,
            &reordered.inventory,
            &reordered.trails,
            &reordered.cases,
            &reordered.failure_funnel,
        )
        .unwrap(),
        expected
    );

    let mut changed_case = parsed.clone();
    changed_case.cases[0].warm_end_to_end_ms += 1;
    assert_ne!(
        contracts::results_evidence_sha256(
            &changed_case.environment,
            &changed_case.inventory,
            &changed_case.trails,
            &changed_case.cases,
            &changed_case.failure_funnel,
        )
        .unwrap(),
        expected
    );

    let mut changed_timestamp = parsed.clone();
    changed_timestamp.environment.recorded_at = "2026-08-21T12:34:57Z".into();
    assert_ne!(
        contracts::results_evidence_sha256(
            &changed_timestamp.environment,
            &changed_timestamp.inventory,
            &changed_timestamp.trails,
            &changed_timestamp.cases,
            &changed_timestamp.failure_funnel,
        )
        .unwrap(),
        expected,
        "the environment timestamp enters only through the results evidence tuple"
    );

    let mut presentation_only = value;
    presentation_only["provenance"]["results_sha256"] = json!(expected);
    presentation_only["decision"] = json!({"ignored":"outside evidence tuple"});
    presentation_only["findings"] = json!("outside evidence tuple");
    assert_eq!(
        contracts::results_evidence_sha256(
            &parsed.environment,
            &parsed.inventory,
            &parsed.trails,
            &parsed.cases,
            &parsed.failure_funnel,
        )
        .unwrap(),
        presentation_only["provenance"]["results_sha256"]
    );
}

#[test]
fn task11a_environment_identity_is_closed_sanitized_and_bound_to_provenance() {
    let mut value = report();
    value["environment"]["qualification_source_commit"] = json!(COMMIT);
    value["environment"]["qualification_source_tree"] = json!(COMMIT);
    value["environment"]["recorded_at"] = json!("2026-08-21T12:34:56Z");
    value["environment"]["invocation"] = json!({
        "binary_name":"codestory-proof-availability",
        "operation":"run",
        "profile":"local_core_only",
        "corpus_sha256":value["provenance"]["corpus_sha256"],
        "thresholds_sha256":value["provenance"]["thresholds_sha256"]
    });
    value["provenance"]["results_sha256"] =
        json!(contracts::results_evidence_sha256_from_json(&value).unwrap());
    QualificationSummaryV1::from_json(value.clone()).expect("bound environment identity");

    for mutation in [
        ("qualification_id", json!("20260821T123456Z-aaaaaaaaaaaa")),
        ("cargo_profile", json!("debug")),
    ] {
        let mut drift = value.clone();
        if mutation.0 == "qualification_id" {
            drift["environment"][mutation.0] = mutation.1;
        } else {
            drift["environment"]["build"][mutation.0] = mutation.1;
        }
        rebind_results_digest(&mut drift);
        assert!(
            QualificationSummaryV1::from_json(drift).is_err(),
            "environment must reject {} drift",
            mutation.0
        );
    }

    let mut summary_identity_drift = value.clone();
    summary_identity_drift["qualification_id"] = json!("20260821T123456Z-bbbbbbbbbbbb");
    assert!(QualificationSummaryV1::from_json(summary_identity_drift).is_err());

    let mut host_drift = value.clone();
    host_drift["environment"]["rust_host"] = json!("x86_64-unknown-linux-gnu");
    rebind_results_digest(&mut host_drift);
    assert!(QualificationSummaryV1::from_json(host_drift).is_err());

    let mut argv_drift = value.clone();
    argv_drift["environment"]["build"]["prescribed_argv"][2] = json!("--debug");
    rebind_results_digest(&mut argv_drift);
    assert!(QualificationSummaryV1::from_json(argv_drift).is_err());

    let mut dirty_build = value.clone();
    dirty_build["environment"]["build"]["source_dirty"] = json!(true);
    rebind_results_digest(&mut dirty_build);
    assert!(QualificationSummaryV1::from_json(dirty_build).is_err());

    let mut build_source_drift = value.clone();
    build_source_drift["environment"]["build"]["source_commit"] =
        json!("cccccccccccccccccccccccccccccccccccccccc");
    rebind_results_digest(&mut build_source_drift);
    assert!(QualificationSummaryV1::from_json(build_source_drift).is_err());

    for (field, bad) in [
        (
            "qualification_source_commit",
            json!("cccccccccccccccccccccccccccccccccccccccc"),
        ),
        (
            "qualification_source_tree",
            json!("dddddddddddddddddddddddddddddddddddddddd"),
        ),
    ] {
        let mut drift = value.clone();
        drift["environment"][field] = bad;
        assert!(
            QualificationSummaryV1::from_json(drift).is_err(),
            "{field} drift"
        );
    }
    for forbidden in [
        "/Users/albert/secret",
        "TOKEN=private",
        "run --corpus /tmp/input.json",
    ] {
        let mut leaked = value.clone();
        leaked["environment"]["os"] = json!(forbidden);
        assert!(
            QualificationSummaryV1::from_json(leaked).is_err(),
            "leak {forbidden}"
        );
    }
    let mut leap_day = value.clone();
    leap_day["environment"]["recorded_at"] = json!("2024-02-29T23:59:59.123Z");
    rebind_results_digest(&mut leap_day);
    QualificationSummaryV1::from_json(leap_day).expect("valid leap-day RFC3339 UTC timestamp");

    for invalid in [
        "2023-02-29T12:34:56Z",
        "2024-02-30T12:34:56Z",
        "2026-04-31T12:34:56Z",
        "2026-00-21T12:34:56Z",
        "2026-13-21T12:34:56Z",
        "2026-08-00T12:34:56Z",
        "2026-08-21T24:00:00Z",
        "2026-08-21T23:60:00Z",
        "2026-08-21T23:59:60Z",
        "2026-08-21T1:02:03Z",
        "2026-08-21T12:34:56+00:00",
    ] {
        let mut malformed = value.clone();
        malformed["environment"]["recorded_at"] = json!(invalid);
        rebind_results_digest(&mut malformed);
        assert!(
            QualificationSummaryV1::from_json(malformed).is_err(),
            "invalid RFC3339 UTC timestamp {invalid}"
        );
    }
}

#[test]
fn task11a_product_result_contract_preserves_every_disposition_and_failure_basis() {
    let cases = [
        json!({"kind":"contract_proven","contract_digest":SHA}),
        json!({"kind":"contract_refuted","contract_digest":SHA,"basis":{"kind":"positive_contradiction","step_index":0,"prohibition_index":0}}),
        json!({"kind":"contract_refuted","contract_digest":SHA,"basis":{"kind":"certified_absence","step_index":0,"extractor_capability_receipt_id":"extractor:1","enumeration_receipt_id":"enumeration:1"}}),
        json!({"kind":"unknown","contract_digest":SHA,"gaps":[{"kind":"direct_call_missing","step_index":0}]}),
        json!({"kind":"unavailable","contract_digest":SHA,"reasons":["source_not_bound_to_publication"]}),
        json!({"kind":"invalid","failure":{"stage":"tool_execution","code":"internal_error"}}),
    ];
    for actual in cases {
        let parsed: contracts::ActualProductResultV1 =
            serde_json::from_value(actual.clone()).expect("closed product result");
        assert_eq!(serde_json::to_value(parsed).unwrap(), actual);
    }

    for (projected, expected_kind, expected_summary_kind) in [
        (
            json!({"kind":"contract_proven","contract_digest":SHA,"receipts":[{"receipt_id":"indexed-call-edge:1","edge_id":"1"}]}),
            "contract_proven",
            "contract_proven",
        ),
        (
            json!({"kind":"contract_refuted","contract_digest":SHA,"refutation":{"kind":"prohibited_scope_traversal","step_index":0,"prohibition_index":0,"connected_receipts":[{"receipt_id":"indexed-call-edge:1","edge_id":"1"}]}}),
            "contract_refuted",
            "unknown",
        ),
        (
            json!({"kind":"contract_refuted","contract_digest":SHA,"refutation":{"kind":"certified_absence","step_index":0,"extractor_capability_receipt_id":"extractor:1","untruncated_enumeration_receipt_id":"enumeration:1","connected_receipts":[]}}),
            "contract_refuted",
            "certified_absence",
        ),
        (
            json!({"kind":"unknown","contract_digest":SHA,"gaps":[{"kind":"direct_call_missing","step_index":0}],"connected_receipts":[]}),
            "unknown",
            "unknown",
        ),
        (
            json!({"kind":"unavailable","contract_digest":SHA,"reasons":["source_not_bound_to_publication"]}),
            "unavailable",
            "unknown",
        ),
    ] {
        let report =
            contracts::product_disposition_from_projection(&json!({"disposition":projected}))
                .expect("actual product projection conversion");
        assert_eq!(
            serde_json::to_value(&report.actual).unwrap()["kind"],
            expected_kind
        );
        assert_eq!(
            serde_json::to_value(&report).unwrap()["kind"],
            expected_summary_kind
        );
    }
    assert_eq!(
        serde_json::to_value(contracts::invalid_contract_report("invalid_contract")).unwrap()["actual"]
            ["failure"]["stage"],
        "contract_validation"
    );
    let observed_failure =
        codestory_runtime::proof_qualification_support::ObservedIntegratedProjectedCallPathResult {
            result: Err(codestory_contracts::api::ApiError::internal(
                "fixture failure",
            )),
            trace: codestory_runtime::proof_qualification_support::ProofQualificationTrace {
                selectors: Vec::new(),
                selector_early_return: false,
                steps: Vec::new(),
                finalization:
                    codestory_runtime::proof_qualification_support::FinalizationTrace::NotRun,
            },
        };
    let failure = contracts::observed_product_disposition_to_report(&observed_failure)
        .expect("observed tool failure conversion");
    assert_eq!(
        serde_json::to_value(failure).unwrap()["actual"]["failure"],
        json!({"stage":"tool_execution","code":"internal"})
    );

    let mut positive_contradiction = report();
    let connected =
        positive_contradiction["cases"][0]["product_disposition"]["actual"]["receipts"].clone();
    positive_contradiction["cases"][0]["product_disposition"]["kind"] = json!("unknown");
    positive_contradiction["cases"][0]["product_disposition"]["actual"] = json!({
        "kind":"contract_refuted",
        "contract_digest":SHA,
        "basis":{"kind":"positive_contradiction","step_index":0,"prohibition_index":0,"connected_receipts":connected}
    });
    rebind_results_digest(&mut positive_contradiction);
    QualificationSummaryV1::from_json(positive_contradiction.clone())
        .expect("positive contradiction retains an exact actual basis");
    positive_contradiction["cases"][0]["product_disposition"]["kind"] = json!("certified_absence");
    rebind_results_digest(&mut positive_contradiction);
    QualificationSummaryV1::from_json(positive_contradiction)
        .expect_err("positive contradiction cannot flatten into certified absence");
}

#[test]
fn task10a_red_path_file_is_the_closed_thirty_path_root() {
    let schema = contracts::schema_json(SchemaDocument::Path);
    assert_eq!(
        schema["properties"]["schema"]["const"],
        contracts::PATH_FILE_SCHEMA
    );
    assert_eq!(schema["properties"]["paths"]["minItems"], 30);
    assert_eq!(schema["properties"]["paths"]["maxItems"], 30);
    assert!(
        schema["properties"]
            .get("cohort_path_file_sha256")
            .is_none()
    );
}

#[test]
fn task10a_red_corpus_identity_uses_external_path_roots() {
    let path_file =
        CohortPathFileV1::from_json(cohort_path_file("codestory-rust")).expect("closed path root");
    let digest = canonical_cohort_path_file_sha256(&path_file).expect("path-file digest");
    assert_eq!(digest.len(), 64);
    let frozen = CorpusV1::from_json(corpus()).expect("corpus references only");
    assert!(
        serde_json::to_value(&frozen)
            .unwrap()
            .get("paths")
            .is_none()
    );
}

#[test]
fn task10a_contracts_reject_translation_mutation_and_range_drift() {
    let base = cohort_path_file("codestory-rust");

    let mut quote = base.clone();
    quote["paths"][0]["clauses"][0]["quote"] = json!("wrong");
    assert!(CohortPathFileV1::from_json(quote).is_err());

    let mut uncovered = base.clone();
    uncovered["paths"][0]["clauses"][0]["end_byte_exclusive"] = json!(5);
    uncovered["paths"][0]["clauses"][0]["quote"] = json!("exact");
    assert!(CohortPathFileV1::from_json(uncovered).is_err());

    let mut guarded_non_material = base.clone();
    guarded_non_material["paths"][0]["clauses"][0]["classification"] =
        json!({"kind":"non_material","reason":"commentary"});
    assert!(CohortPathFileV1::from_json(guarded_non_material).is_err());

    let mut missing_field = base.clone();
    missing_field["paths"][0]["clauses"][0]["classification"]["fields"]
        .as_array_mut()
        .unwrap()
        .retain(|field| field["kind"] != "directness");
    assert!(CohortPathFileV1::from_json(missing_field).is_err());

    let mut out_of_range_field = base.clone();
    out_of_range_field["paths"][0]["clauses"][0]["classification"]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"step_target","step":6}));
    assert!(CohortPathFileV1::from_json(out_of_range_field).is_err());

    let mut bad_path = base.clone();
    bad_path["paths"][0]["spec"]["start"]["project_file_components"] =
        json!(["src", "..", "escape.rs"]);
    assert!(CohortPathFileV1::from_json(bad_path).is_err());

    let mut bad_source_path = base.clone();
    bad_source_path["paths"][0]["oracle_steps"][0]["callsite_expression"]["path"] =
        json!("src/../escape.rs");
    assert!(CohortPathFileV1::from_json(bad_source_path).is_err());

    let mut unchanged_mutation = base.clone();
    unchanged_mutation["paths"][0]["negative_mutations"][0]["mutated_spec"] =
        unchanged_mutation["paths"][0]["spec"].clone();
    assert!(CohortPathFileV1::from_json(unchanged_mutation).is_err());

    let mut multi_coordinate_mutation = base.clone();
    multi_coordinate_mutation["paths"][0]["negative_mutations"][0]["mutated_spec"]["exclude_from_projection"] =
        json!([{"kind":"canonical_id","canonical_id":"extra"}]);
    assert!(CohortPathFileV1::from_json(multi_coordinate_mutation).is_err());

    let mut expression_outside_line = base;
    expression_outside_line["paths"][0]["oracle_steps"][0]["callsite_expression"]["start_byte"] =
        json!(0);
    assert!(CohortPathFileV1::from_json(expression_outside_line).is_err());
}

#[test]
fn task10a_cohort_and_corpus_invariants_are_derived() {
    let mut file_cap = cohort_path_file("codestory-rust");
    for path in file_cap["paths"].as_array_mut().unwrap().iter_mut().take(7) {
        path["oracle_steps"][0]["caller"]["range"]["path"] = json!("src/one.rs");
    }
    assert!(CohortPathFileV1::from_json(file_cap).is_err());

    let mut too_few_areas = cohort_path_file("codestory-rust");
    for path in too_few_areas["paths"].as_array_mut().unwrap() {
        path["audit"]["source_area"] = json!("one-area");
    }
    assert!(CohortPathFileV1::from_json(too_few_areas).is_err());

    let mut unavailable_areas = cohort_path_file("codestory-rust");
    unavailable_areas["source_area_requirement"] =
        json!({"kind":"not_available","reason":"upstream has no stable area taxonomy"});
    for path in unavailable_areas["paths"].as_array_mut().unwrap() {
        path["audit"]["source_area"] = json!("unclassified");
    }
    CohortPathFileV1::from_json(unavailable_areas)
        .expect("a documented unavailable source-area taxonomy is explicit");

    let mut duplicate_relation = cohort_path_file("codestory-rust");
    let path = &mut duplicate_relation["paths"][10];
    let repeated = json!({"kind":"canonical_id","canonical_id":"repeated-relation"});
    path["spec"]["start"] = repeated.clone();
    path["spec"]["steps"][0]["target"] = repeated.clone();
    path["spec"]["steps"][1]["target"] = repeated.clone();
    for step in path["oracle_steps"].as_array_mut().unwrap() {
        step["caller"]["selector"] = repeated.clone();
        step["target"]["selector"] = repeated.clone();
    }
    let positive = path["spec"].clone();
    let alternate_target =
        path["negative_mutations"][0]["source_audit"]["target"]["selector"].clone();
    let mut target_mutation = positive.clone();
    target_mutation["steps"][0]["target"] = alternate_target;
    path["negative_mutations"][0]["mutated_spec"] = target_mutation;
    path["negative_mutations"][0]["source_audit"]["caller"]["selector"] = repeated.clone();
    let alternate_source =
        path["negative_mutations"][1]["source_audit"]["caller"]["selector"].clone();
    let mut source_mutation = positive;
    source_mutation["start"] = alternate_source;
    path["negative_mutations"][1]["mutated_spec"] = source_mutation;
    path["negative_mutations"][1]["source_audit"]["target"]["selector"] = repeated;
    assert!(CohortPathFileV1::from_json(duplicate_relation).is_err());

    let mut root_same_reviewer = cohort_path_file("codestory-rust");
    root_same_reviewer["reviewer"] = root_same_reviewer["curator"].clone();
    assert!(CohortPathFileV1::from_json(root_same_reviewer).is_err());

    let mut child_same_reviewer = cohort_path_file("codestory-rust");
    child_same_reviewer["paths"][0]["audit"]["reviewer"] =
        child_same_reviewer["paths"][0]["audit"]["curator"].clone();
    assert!(CohortPathFileV1::from_json(child_same_reviewer).is_err());

    let frozen = CorpusV1::from_json(corpus()).unwrap();
    let mut files = parsed_path_files();
    files.pop();
    assert!(frozen.validate_with_path_files(&files).is_err());
    let mut files = parsed_path_files();
    files.push(CohortPathFileV1::from_json(cohort_path_file("gin-go")).unwrap());
    assert!(frozen.validate_with_path_files(&files).is_err());
    let mut files = parsed_path_files();
    files[0].source_tree_sha256 =
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into();
    assert!(frozen.validate_with_path_files(&files).is_err());

    let mut files = parsed_path_files();
    let duplicate = files[0].paths[0].case_id.clone();
    files[1].paths[0].case_id = duplicate.clone();
    for mutation in &mut files[1].paths[0].negative_mutations {
        mutation.path_id = duplicate.clone();
    }
    let mut rebound = frozen.clone();
    rebound.cohorts[1].path_file_sha256 = canonical_cohort_path_file_sha256(&files[1]).unwrap();
    assert!(rebound.validate_with_path_files(&files).is_err());

    let mut swapped_hashes = corpus();
    let left = swapped_hashes["cohorts"][0]["path_file_sha256"].clone();
    swapped_hashes["cohorts"][0]["path_file_sha256"] =
        swapped_hashes["cohorts"][1]["path_file_sha256"].clone();
    swapped_hashes["cohorts"][1]["path_file_sha256"] = left;
    assert!(
        CorpusV1::from_json(swapped_hashes)
            .unwrap()
            .validate_with_path_files(&parsed_path_files())
            .is_err()
    );
}

#[test]
fn task10a_overlapping_resolved_anchors_and_individual_audits_are_legal() {
    let mut value = cohort_path_file("codestory-rust");
    let source = value["paths"][0]["source_text"]
        .as_str()
        .unwrap()
        .to_owned();
    value["paths"][0]["clauses"].as_array_mut().unwrap().push(json!({
        "clause_id":"overlap","start_byte":6,"end_byte_exclusive":12,
        "quote":&source[6..12],"classification":{"kind":"resolved_material","fields":[{"kind":"directness","step":0}]}
    }));
    value["paths"][0]["clauses"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "clause_id":"overlap-non-material","start_byte":6,"end_byte_exclusive":12,
            "quote":&source[6..12],"classification":{"kind":"non_material","reason":"commentary"}
        }));
    value["paths"][0]["audit"]["curator"] = json!("different-curator@example.invalid");
    CohortPathFileV1::from_json(value).expect("overlap and per-path audit identity remain legal");
}

#[test]
fn task10a_wire_mirrors_keep_all_dark_selector_classification_and_field_variants_closed() {
    for selector in [
        json!({"kind":"pinned_node","project_id":"p","core_generation_id":"g","core_run_id":"r","node_id":"1"}),
        json!({"kind":"canonical_id","canonical_id":"canonical"}),
        json!({"kind":"qualified_name","qualified_name":"crate::item","project_file_components":["src","lib.rs"]}),
    ] {
        serde_json::from_value::<ExactSymbolSelectorV1>(selector.clone()).unwrap();
        serde_json::from_value::<ExactScopeSelectorV1>(selector).unwrap();
    }
    for classification in [
        json!({"kind":"resolved_material","fields":[{"kind":"start"}]}),
        json!({"kind":"unresolved_material","reason":"unsupported_interpretation"}),
        json!({"kind":"non_material","reason":"connector"}),
    ] {
        serde_json::from_value::<ClauseClassificationV1>(classification).unwrap();
    }
    for field in [
        json!({"kind":"start"}),
        json!({"kind":"step_target","step":0}),
        json!({"kind":"directness","step":0}),
        json!({"kind":"ordering","step":0}),
        json!({"kind":"relation","step":0}),
        json!({"kind":"traversal_prohibition","index":0}),
        json!({"kind":"projection_exclusion","index":0}),
    ] {
        serde_json::from_value::<ProofContractFieldV1>(field).unwrap();
    }
}

#[test]
fn task10a_oracle_validation_uses_every_product_clause_guard_family() {
    for guarded in [
        "'Alpha'",
        "invokes",
        "immediately",
        "fifth 12th",
        "only",
        "prohibits avoid",
        "lib.ts",
        "package.Symbol",
    ] {
        let mut value = cohort_path_file("codestory-rust");
        let fields = value["paths"][0]["clauses"][0]["classification"]["fields"].clone();
        let source = format!("x {guarded}");
        value["paths"][0]["source_text"] = json!(source);
        value["paths"][0]["clauses"] = json!([
            {
                "clause_id":"resolved-fields",
                "start_byte":0,
                "end_byte_exclusive":1,
                "quote":"x",
                "classification":{"kind":"resolved_material","fields":fields},
            },
            {
                "clause_id":"reviewer-example",
                "start_byte":1,
                "end_byte_exclusive":source.len(),
                "quote":&source[1..],
                "classification":{"kind":"non_material","reason":"commentary"},
            },
        ]);
        assert!(
            CohortPathFileV1::from_json(value).is_err(),
            "product guard family accepted as non-material: {guarded:?}"
        );
    }
}

fn range_at(path: &str, start: u64, end: u64) -> Value {
    json!({"path":path,"start_byte":start,"end_byte":end,"file_byte_length":4096,"sha256":SHA})
}

fn receipt_line_range_at(path: &str, start: u64, end: u64) -> Value {
    assert_eq!(end - start, 8);
    json!({
        "path":path,
        "start_byte":start,
        "end_byte":end,
        "file_byte_length":4096,
        "sha256":format!("{:x}", Sha256::digest(b"call();\n")),
    })
}

fn selector(symbol: &str, path: &str) -> Value {
    json!({"kind":"qualified_name","qualified_name":symbol,"project_file_components":path.split('/').collect::<Vec<_>>()})
}

fn declaration(symbol: &str, path: &str, start: u64, end: u64) -> Value {
    json!({"symbol":symbol,"selector":selector(symbol,path),"range":range_at(path,start,end)})
}

fn path(case_id: &str, step_count: u8, ordinal: usize) -> Value {
    let source_path = format!("src/area{}/file{}.rs", ordinal % 5, ordinal % 5);
    let start_symbol = format!("{case_id}::start");
    let oracle_steps = (0..step_count)
        .map(|index| {
            let start = u64::from(index) * 40;
            let caller_symbol = if index == 0 {
                start_symbol.clone()
            } else {
                format!("{case_id}::target_{}", index - 1)
            };
            let caller_start = if index == 0 { 0 } else { start - 20 };
            let caller_end = if index == 0 { 10 } else { start - 8 };
            json!({
              "caller":declaration(&caller_symbol,&source_path,caller_start,caller_end),
              "callsite_line":index + 1,
              "callsite_expression":range_at(&source_path,start + 12,start + 18),
              "receipt_line_window":receipt_line_range_at(&source_path,start + 11,start + 19),
              "receipt_file_sha256":SHA,
              "target":declaration(&format!("{case_id}::target_{index}"),&source_path,start + 20,start + 32)
            })
        })
        .collect::<Vec<_>>();
    let steps = (0..step_count)
        .map(|index| json!({"target":selector(&format!("{case_id}::target_{index}"),&source_path)}))
        .collect::<Vec<_>>();
    let spec = json!({
        "start":selector(&start_symbol,&source_path),
        "steps":steps,
        "prohibit_traversal_through":[],
        "exclude_from_projection":[]
    });
    let mut fields = vec![json!({"kind":"start"})];
    for step in 0..step_count {
        for kind in ["step_target", "directness", "ordering", "relation"] {
            fields.push(json!({"kind":kind,"step":step}));
        }
    }
    let source_text = "exact direct ordered call path";
    let alternate_target = format!("{case_id}::absent_target");
    let alternate_source = format!("{case_id}::absent_source");
    let mut target_spec = spec.clone();
    target_spec["steps"][0]["target"] = selector(&alternate_target, &source_path);
    let mut source_spec = spec.clone();
    source_spec["start"] = selector(&alternate_source, &source_path);
    let first_target = format!("{case_id}::target_0");
    json!({
      "case_id":case_id, "language":"rust", "source_text":source_text,
      "clauses":[{"clause_id":"c1","start_byte":0,"end_byte_exclusive":source_text.len(),"quote":source_text,"classification":{"kind":"resolved_material","fields":fields}}],
      "spec":spec,
      "oracle_steps":oracle_steps,
      "negative_mutations":[
        {"mutation_id":format!("{case_id}-target"),"path_id":case_id,"kind":"replace_step_target","step_index":0,"mutated_spec":target_spec,"source_audit":{"caller":declaration(&start_symbol,&source_path,0,10),"target":declaration(&alternate_target,&source_path,300,312),"caller_body":range_at(&source_path,0,320),"finding":"no_direct_call"}},
        {"mutation_id":format!("{case_id}-source"),"path_id":case_id,"kind":"replace_step_source","step_index":0,"mutated_spec":source_spec,"source_audit":{"caller":declaration(&alternate_source,&source_path,320,332),"target":declaration(&first_target,&source_path,20,32),"caller_body":range_at(&source_path,300,360),"finding":"no_direct_call"}}],
      "audit":{"source_area":format!("area-{}",ordinal % 5),"curator":"path-curator@example.invalid","reviewer":"path-reviewer@example.invalid","review_date":"2026-08-21"}
    })
}

fn registry(id: &str) -> (&'static str, &'static str, &'static str) {
    contracts::QUALIFICATION_REPOSITORIES
        .iter()
        .find(|entry| entry.0 == id)
        .map(|entry| (entry.1, entry.2, entry.3))
        .expect("registry id")
}

fn cohort_path_file(id: &str) -> Value {
    let (repository, commit, workspace) = registry(id);
    let lengths = [10u8, 7, 5, 3, 3, 2];
    let mut ordinal = 0usize;
    let paths = lengths
        .iter()
        .enumerate()
        .flat_map(|(length, count)| (0..*count).map(move |index| (length, index)))
        .map(|(length, index)| {
            let value = path(
                &format!("{id}-l{}-{index}", length + 1),
                (length + 1) as u8,
                ordinal,
            );
            ordinal += 1;
            value
        })
        .collect::<Vec<_>>();
    json!({
        "schema":contracts::PATH_FILE_SCHEMA,"repository_id":id,"repository":repository,
        "commit":commit,"workspace":workspace,"source_tree_sha256":SHA,
        "curator":"cohort-curator@example.invalid","reviewer":"cohort-reviewer@example.invalid","review_date":"2026-08-21",
        "source_area_requirement":{"kind":"required_at_least_five"},"paths":paths
    })
}

fn path_files() -> Vec<Value> {
    ["codestory-rust", "vite-ts-js", "flask-python", "gin-go"]
        .into_iter()
        .map(cohort_path_file)
        .collect()
}

fn parsed_path_files() -> Vec<CohortPathFileV1> {
    path_files()
        .into_iter()
        .map(|value| CohortPathFileV1::from_json(value).expect("path file"))
        .collect()
}

pub(crate) fn corpus() -> Value {
    let files = parsed_path_files();
    let threshold_hash =
        canonical_thresholds_sha256(&ThresholdsV1::from_json(thresholds()).expect("thresholds"))
            .expect("canonical thresholds hash");
    json!({
      "schema":"codestory.proof-availability-corpus/v1","corpus_id":"proof-availability-v1","thresholds_sha256":threshold_hash,"methodology_sha256":SHA,"curator":"curator@example.invalid","reviewer":"reviewer@example.invalid","review_date":"2026-08-21",
      "cohorts":files.iter().map(|file|json!({"repository_id":file.repository_id,"repository":file.repository,"commit":file.commit,"workspace":file.workspace,"path_file":format!("paths/{}.json",file.repository_id),"path_file_sha256":canonical_cohort_path_file_sha256(file).unwrap(),"source_tree_sha256":file.source_tree_sha256,"path_count":30,"positive_step_count":78,"path_length_distribution":[{"path_length":1,"path_count":10},{"path_length":2,"path_count":7},{"path_length":3,"path_count":5},{"path_length":4,"path_count":3},{"path_length":5,"path_count":3},{"path_length":6,"path_count":2}]})).collect::<Vec<_>>(),
      "positive_request_count":120,"positive_step_count":312,"negative_request_count":240
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

pub(crate) fn report() -> Value {
    let frozen = corpus();
    let files = path_files();
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
    let oracle_cases = files
        .iter()
        .flat_map(|file| {
            let repository_id = file["repository_id"].as_str().expect("repository id");
            file["paths"]
                .as_array()
                .expect("paths")
                .iter()
                .map(move |path| (repository_id, path))
        })
        .collect::<Vec<_>>();
    let cases = oracle_cases
        .into_iter()
        .enumerate()
        .map(|(case_index, (repository_id, path))| {
            let oracle_path: contracts::OraclePathV1 =
                serde_json::from_value(path.clone()).expect("oracle path");
            let contract_digest =
                contracts::expected_contract_digest_for_oracle_path(&oracle_path)
                    .expect("product contract digest");
            let attempted = path["spec"]["steps"].as_array().expect("steps").len() as u64;
            let case_id = path["case_id"].as_str().expect("case id");
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
                    let callsite = &step["receipt_line_window"];
                    let start = callsite["start_byte"].as_u64().expect("callsite start");
                    let end = callsite["end_byte"].as_u64().expect("callsite end");
                    let source_path = callsite["path"].as_str().expect("source path");
                    let oracle_step = json!({
                        "caller":step["caller"],
                        "callsite_line":step["callsite_line"],
                        "receipt_line_window":step["receipt_line_window"],
                        "receipt_file_sha256":step["receipt_file_sha256"],
                        "target":step["target"]
                    });
                    let source_node_id = -(i64::try_from(step_index).expect("step index") + 1);
                    let target_node_id = source_node_id - 1;
                    let source_canonical_id =
                        format!("canonical-{case_index}-{source_node_id}");
                    let target_canonical_id =
                        format!("canonical-{case_index}-{target_node_id}");
                    let source_pinned = contracts::PinnedNodeIdentityV1 {
                        project_id: project_id.clone(),
                        core_generation_id: core_generation_id.clone(),
                        core_run_id: core_run_id.clone(),
                        node_id: source_node_id.to_string(),
                    };
                    let target_pinned = contracts::PinnedNodeIdentityV1 {
                        project_id: project_id.clone(),
                        core_generation_id: core_generation_id.clone(),
                        core_run_id: core_run_id.clone(),
                        node_id: target_node_id.to_string(),
                    };
                    json!({
                        "receipt_id":format!("indexed-call-edge:fixture-{case_index}-{step_index}"),
                        "step_index":step_index,
                        "edge_id":edge_id,
                        "source":{
                            "pinned":{"project_id":project_id,"core_generation_id":core_generation_id,"core_run_id":core_run_id,"node_id":source_node_id.to_string()},
                            "canonical_id_binding_sha256":contracts::resolved_canonical_id_binding_sha256(&source_pinned, &source_canonical_id).unwrap(),
                            "qualified_name":step["caller"]["symbol"],
                            "project_file_components":source_path.split('/').collect::<Vec<_>>()
                        },
                        "target":{
                            "pinned":{"project_id":project_id,"core_generation_id":core_generation_id,"core_run_id":core_run_id,"node_id":target_node_id.to_string()},
                            "canonical_id_binding_sha256":contracts::resolved_canonical_id_binding_sha256(&target_pinned, &target_canonical_id).unwrap(),
                            "qualified_name":step["target"]["symbol"],
                            "project_file_components":source_path.split('/').collect::<Vec<_>>()
                        },
                        "certainty":"certain",
                        "callsite_identity":format!("{file_node_id}:{}:0:{target_node_id}|fixture", step_index + 1),
                        "callsite_line":step_index + 1,
                        "containment":{"file_node_id":file_node_id,"owner_node_id":source_node_id,"start_line":1,"end_line":attempted},
                        "line_window":{
                            "kind":"indexed_line_v1",
                            "project_file_components":source_path.split('/').collect::<Vec<_>>(),
                            "byte_start":start,
                            "byte_end":end,
                            "indexed_sha256":SHA,
                            "observed_sha256":SHA,
                            "text":"call();\n"
                        },
                        "oracle_comparison":{"kind":"exact","oracle_step_index":step_index,"oracle_step":oracle_step}
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
            let projected_receipts = observed_receipts
                .iter()
                .map(|receipt| json!({
                    "receipt_id":receipt["receipt_id"],
                    "edge_id":receipt["edge_id"].as_i64().unwrap().to_string()
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
                    "mutated_spec":mutation["mutated_spec"],
                    "contract_proven":false
                }))
                .collect::<Vec<_>>();
            json!({
                "case_id":case_id,"repository_id":repository_id,
                "product_disposition":{"kind":"contract_proven","gaps":[],"authoritative_receipts":authoritative_receipts,"actual":{"kind":"contract_proven","contract_digest":contract_digest,"receipts":projected_receipts}},
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
    let mut report = json!({
      "schema":"codestory.proof-availability-report/v2","qualification_id":"20260821T000000Z-bbbbbbbbbbbb",
      "provenance":{"source_commit":COMMIT,"source_tree":COMMIT,"binary_sha256":SHA,"corpus_sha256":corpus_hash,"thresholds_sha256":threshold_hash,"results_sha256":SHA},
      "environment":{"qualification_id":"20260821T000000Z-bbbbbbbbbbbb","environment_id":"macos-arm64","os":"macos","architecture":"aarch64","rust_host":"aarch64-apple-darwin","binary_sha256":SHA,"qualification_source_commit":COMMIT,"qualification_source_tree":COMMIT,"recorded_at":"2026-08-21T12:34:56Z","build":{"source_commit":COMMIT,"source_tree":COMMIT,"source_dirty":false,"rustc_vv":"rustc 1.91.0\nbinary: rustc\nhost: aarch64-apple-darwin\n","cargo_profile":"release","prescribed_argv":["cargo","build","--release","--locked","-p","codestory-bench","--bin","codestory-proof-availability"]},"invocation":{"binary_name":"codestory-proof-availability","operation":"run","profile":"local_core_only","corpus_sha256":corpus_hash,"thresholds_sha256":threshold_hash},"projects":frozen["cohorts"].as_array().unwrap().iter().map(|cohort|{let id=cohort["repository_id"].as_str().unwrap();json!({"repository_id":id,"source_head":cohort["commit"],"source_tree":SHA,"store_schema":"codestory-store/v1","file_count":10,"node_count":20,"edge_count":30,"freshness":"fresh","database_sha256":SHA,"core_generation":1,"identity":{"project_id":format!("project-{id}"),"core_generation_id":format!("generation-{id}"),"core_run_id":format!("run-{id}")}})}).collect::<Vec<_>>()},
      "inventory":cohort_ids.iter().map(|id|json!({"repository_id":id,"stored_call_rows":"10","effective_endpoint_rows":"10","exact_resolved_rows":"8","admitted_rows":"7","unresolved_placeholder_rows":"2"})).collect::<Vec<_>>(),
      "trails":cohort_ids.iter().map(|id|json!({"repository_id":id,"lengths":[{"length":1,"effective_endpoint":"10","exact_resolved":"8","strictly_admitted":"7"},{"length":2,"effective_endpoint":"9","exact_resolved":"7","strictly_admitted":"6"},{"length":3,"effective_endpoint":"8","exact_resolved":"6","strictly_admitted":"5"},{"length":4,"effective_endpoint":"7","exact_resolved":"5","strictly_admitted":"4"},{"length":5,"effective_endpoint":"6","exact_resolved":"4","strictly_admitted":"3"},{"length":6,"effective_endpoint":"5","exact_resolved":"3","strictly_admitted":"2"}]})).collect::<Vec<_>>(),
      "cases":cases,
      "failure_funnel":{"attempted_positive_steps":312,"classified_positive_steps":312,"unclassified_positive_steps":0,"buckets":[{"outcome":{"kind":"admitted"},"count":"312"}]}
    });
    report["provenance"]["results_sha256"] =
        json!(contracts::results_evidence_sha256_from_json(&report).unwrap());
    report
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
    let frozen = CorpusV1::from_json(corpus()).expect("maximal frozen corpus");
    frozen
        .validate_with_path_files(&parsed_path_files())
        .expect("four bound path roots");
    let mut unknown = corpus();
    unknown["unknown"] = json!(true);
    assert!(CorpusV1::from_json(unknown).is_err());
    let mut commit = corpus();
    commit["cohorts"][0]["commit"] = json!("abc");
    assert!(CorpusV1::from_json(commit).is_err());
    let mut hash = cohort_path_file("codestory-rust");
    hash["paths"][0]["oracle_steps"][0]["receipt_line_window"]["sha256"] = json!("");
    assert!(CohortPathFileV1::from_json(hash).is_err());
    let mut mutations = cohort_path_file("codestory-rust");
    mutations["paths"][0]["negative_mutations"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(CohortPathFileV1::from_json(mutations).is_err());
    let mut missing_path = cohort_path_file("codestory-rust");
    missing_path["paths"].as_array_mut().unwrap().pop();
    assert!(CohortPathFileV1::from_json(missing_path).is_err());
    let mut duplicate_kind = cohort_path_file("codestory-rust");
    duplicate_kind["paths"][0]["negative_mutations"][1]["kind"] = json!("replace_step_target");
    assert!(CohortPathFileV1::from_json(duplicate_kind).is_err());
}

#[test]
fn oracle_steps_close_over_the_exact_receipt_source_file_hash() {
    let mut value = cohort_path_file("codestory-rust");
    for path in value["paths"].as_array_mut().expect("paths") {
        for step in path["oracle_steps"].as_array_mut().expect("oracle steps") {
            step["receipt_file_sha256"] = json!(SHA);
        }
    }

    let parsed = CohortPathFileV1::from_json(value).expect("full-file-bound oracle steps");
    let round_trip = serde_json::to_value(parsed).expect("oracle JSON");
    assert_eq!(
        round_trip["paths"][0]["oracle_steps"][0]["receipt_file_sha256"],
        SHA
    );
}

#[test]
fn equal_runtime_file_hashes_that_disagree_with_the_oracle_are_rejected() {
    let mut value = report();
    let wrong = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    value["cases"][0]["receipt_evidence"]["observed_receipts"][0]["line_window"]["indexed_sha256"] =
        json!(wrong);
    value["cases"][0]["receipt_evidence"]["observed_receipts"][0]["line_window"]["observed_sha256"] =
        json!(wrong);
    rebind_results_digest(&mut value);

    QualificationSummaryV1::from_json(value)
        .expect_err("equal runtime hashes must still match the independently frozen file hash");
}

#[test]
fn runtime_receipt_comparison_uses_the_oracle_file_hash_not_hash_self_agreement() {
    let path_file =
        CohortPathFileV1::from_json(cohort_path_file("codestory-rust")).expect("oracle path file");
    let oracle = &path_file.paths[0].oracle_steps[0];
    let project_file_components = oracle
        .receipt_line_window
        .path
        .split('/')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let pinned = |node_id: &str| codestory_agent::proof_qualification_support::PinnedNodeIdentity {
        project_id: "project".into(),
        core_generation_id: "generation".into(),
        core_run_id: "run".into(),
        node_id: node_id.into(),
    };
    let identity = |node_id: &str, declaration: &contracts::OracleDeclarationV1| {
        codestory_agent::proof_qualification_support::ResolvedNodeIdentity {
            pinned: pinned(node_id),
            canonical_id: format!("canonical-{node_id}"),
            qualified_name: declaration.symbol.clone(),
            project_file_components: project_file_components.clone(),
        }
    };
    let wrong = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let receipt = codestory_agent::proof_qualification_support::IndexedCallEdgeReceipt {
        receipt: codestory_agent::proof_qualification_support::ReceiptRef {
            receipt_id: "indexed-call-edge:fixture".into(),
            edge_id: "1".into(),
        },
        source: identity("1", &oracle.caller),
        target: identity("2", &oracle.target),
        certainty: codestory_contracts::graph::ResolutionCertainty::Certain,
        callsite_identity: "1:1:0:2|fixture".into(),
        containment: codestory_agent::proof_qualification_support::CallableContainmentEvidence {
            file_node_id: codestory_contracts::graph::NodeId(10),
            owner_node_id: codestory_contracts::graph::NodeId(1),
            start_line: 1,
            end_line: 1,
        },
        line_window: codestory_agent::proof_qualification_support::IndexedLineWindow {
            kind: "indexed_line_v1",
            project_file_components,
            indexed_sha256: wrong.into(),
            observed_sha256: wrong.into(),
            anchor_line: oracle.callsite_line,
            byte_start: usize::try_from(oracle.receipt_line_window.start_byte).unwrap(),
            byte_end: usize::try_from(oracle.receipt_line_window.end_byte).unwrap(),
            text: "call();\n".into(),
        },
    };

    assert!(matches!(
        contracts::compare_task6_receipt_to_oracle(0, &receipt, oracle)
            .expect("closed comparison"),
        ReceiptOracleComparisonV1::Mismatched { mismatches, .. }
            if mismatches == [contracts::ReceiptMismatchFieldV1::CallsiteWindow]
    ));
}

#[test]
fn resolved_canonical_id_bindings_cover_host_paths_relative_ids_and_context() {
    use codestory_agent::proof_qualification_support::{PinnedNodeIdentity, ResolvedNodeIdentity};

    let raws = [
        "/Users/private/worktree/src/caller.rs::caller",
        r"C:\private\worktree\src\caller.rs::caller",
        r"\\server\share\worktree\src\caller.rs::caller",
        r"\\?\C:\private\worktree\src\caller.rs::caller",
        "flask/app.py::dispatch_request",
    ];
    let public_pinned = |node_id: &str| contracts::PinnedNodeIdentityV1 {
        project_id: "project".into(),
        core_generation_id: "generation".into(),
        core_run_id: "run".into(),
        node_id: node_id.into(),
    };
    for raw in raws {
        for node_id in ["-1", "-2"] {
            let product = ResolvedNodeIdentity {
                pinned: PinnedNodeIdentity {
                    project_id: "project".into(),
                    core_generation_id: "generation".into(),
                    core_run_id: "run".into(),
                    node_id: node_id.into(),
                },
                canonical_id: raw.into(),
                qualified_name: "module::callable".into(),
                project_file_components: vec!["src".into(), "caller.rs".into()],
            };
            let public = contracts::ResolvedNodeIdentityV1::try_from(&product).unwrap();
            assert_eq!(public.canonical_id_binding_sha256.len(), 64);
            assert!(
                public
                    .canonical_id_binding_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert!(!serde_json::to_string(&public).unwrap().contains(raw));
        }
    }

    let same_value_first =
        contracts::resolved_canonical_id_binding_sha256(&public_pinned("-1"), "same-canonical-id")
            .unwrap();
    let same_value_second =
        contracts::resolved_canonical_id_binding_sha256(&public_pinned("-2"), "same-canonical-id")
            .unwrap();
    let different_value_same_pin = contracts::resolved_canonical_id_binding_sha256(
        &public_pinned("-1"),
        "different-canonical-id",
    )
    .unwrap();
    assert_ne!(same_value_first, same_value_second);
    assert_ne!(same_value_first, different_value_same_pin);

    let empty = ResolvedNodeIdentity {
        pinned: PinnedNodeIdentity {
            project_id: "project".into(),
            core_generation_id: "generation".into(),
            core_run_id: "run".into(),
            node_id: "-1".into(),
        },
        canonical_id: String::new(),
        qualified_name: "module::callable".into(),
        project_file_components: vec!["src".into(), "caller.rs".into()],
    };
    contracts::ResolvedNodeIdentityV1::try_from(&empty)
        .expect_err("an empty raw canonical ID cannot produce a public binding");
}

#[test]
fn canonical_selector_oracles_recompute_the_contextual_receipt_binding() {
    use codestory_agent::proof_qualification_support::{
        CallableContainmentEvidence, IndexedCallEdgeReceipt, IndexedLineWindow, PinnedNodeIdentity,
        ReceiptRef, ResolvedNodeIdentity,
    };
    use codestory_contracts::graph::{NodeId, ResolutionCertainty};

    let path_file =
        CohortPathFileV1::from_json(cohort_path_file("codestory-rust")).expect("oracle path file");
    let mut oracle = path_file.paths[0].oracle_steps[0].clone();
    let source_raw = "flask/app.py::dispatch_request";
    let target_raw = r"\\server\share\target.py::target";
    oracle.caller.selector = ExactSymbolSelectorV1::CanonicalId {
        canonical_id: source_raw.into(),
    };
    oracle.target.selector = ExactSymbolSelectorV1::CanonicalId {
        canonical_id: target_raw.into(),
    };
    let identity = |node_id: &str, canonical_id: &str, qualified_name: &str| ResolvedNodeIdentity {
        pinned: PinnedNodeIdentity {
            project_id: "project".into(),
            core_generation_id: "generation".into(),
            core_run_id: "run".into(),
            node_id: node_id.into(),
        },
        canonical_id: canonical_id.into(),
        qualified_name: qualified_name.into(),
        project_file_components: oracle
            .receipt_line_window
            .path
            .split('/')
            .map(ToOwned::to_owned)
            .collect(),
    };
    let receipt = IndexedCallEdgeReceipt {
        receipt: ReceiptRef {
            receipt_id: "indexed-call-edge:canonical-selector".into(),
            edge_id: "-42".into(),
        },
        source: identity("-1", source_raw, &oracle.caller.symbol),
        target: identity("-2", target_raw, &oracle.target.symbol),
        certainty: ResolutionCertainty::Certain,
        callsite_identity: format!("-3:{}:0:-2|fixture", oracle.callsite_line),
        containment: CallableContainmentEvidence {
            file_node_id: NodeId(-3),
            owner_node_id: NodeId(-1),
            start_line: oracle.callsite_line,
            end_line: oracle.callsite_line,
        },
        line_window: IndexedLineWindow {
            kind: "indexed_line_v1",
            project_file_components: oracle
                .receipt_line_window
                .path
                .split('/')
                .map(ToOwned::to_owned)
                .collect(),
            indexed_sha256: oracle.receipt_file_sha256.clone(),
            observed_sha256: oracle.receipt_file_sha256.clone(),
            anchor_line: oracle.callsite_line,
            byte_start: usize::try_from(oracle.receipt_line_window.start_byte).unwrap(),
            byte_end: usize::try_from(oracle.receipt_line_window.end_byte).unwrap(),
            text: "call();\n".into(),
        },
    };

    assert!(matches!(
        contracts::compare_task6_receipt_to_oracle(0, &receipt, &oracle).unwrap(),
        ReceiptOracleComparisonV1::Exact { .. }
    ));
    oracle.target.selector = ExactSymbolSelectorV1::CanonicalId {
        canonical_id: format!("{target_raw}x"),
    };
    assert!(matches!(
        contracts::compare_task6_receipt_to_oracle(0, &receipt, &oracle).unwrap(),
        ReceiptOracleComparisonV1::Mismatched { mismatches, .. }
            if mismatches == [contracts::ReceiptMismatchFieldV1::Target]
    ));
}

#[test]
fn resolved_receipt_bindings_reject_legacy_and_malformed_shapes_and_bind_results_digest() {
    let base = report();
    let base_digest = contracts::results_evidence_sha256_from_json(&base).unwrap();
    let binding = base["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]
        ["canonical_id_binding_sha256"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut changed = base.clone();
    let replacement = if binding.starts_with('a') { 'b' } else { 'a' };
    changed["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]["canonical_id_binding_sha256"] =
        json!(format!("{replacement}{}", &binding[1..]));
    assert_ne!(
        contracts::results_evidence_sha256_from_json(&changed).unwrap(),
        base_digest
    );

    let mut canonical_bound = base.clone();
    canonical_bound["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]
        ["oracle_step"]["caller"]["selector"] =
        json!({"kind":"canonical_id","canonical_id":"canonical-0--1"});
    rebind_results_digest(&mut canonical_bound);
    QualificationSummaryV1::from_json(canonical_bound.clone())
        .expect("the exact frozen canonical selector recomputes the receipt commitment");
    canonical_bound["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]
        ["canonical_id_binding_sha256"] = changed["cases"][0]["receipt_evidence"]
        ["observed_receipts"][0]["source"]["canonical_id_binding_sha256"]
        .clone();
    rebind_results_digest(&mut canonical_bound);
    QualificationSummaryV1::from_json(canonical_bound)
        .expect_err("a one-byte commitment mutation cannot satisfy a frozen canonical selector");

    for malformed in [String::new(), "A".repeat(64), "a".repeat(63)] {
        let mut value = base.clone();
        value["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]["canonical_id_binding_sha256"] =
            json!(malformed);
        rebind_results_digest(&mut value);
        QualificationSummaryV1::from_json(value)
            .expect_err("malformed public canonical-ID commitments fail closed");
    }

    let mut legacy = base;
    let source = legacy["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]
        .as_object_mut()
        .unwrap();
    source.remove("canonical_id_binding_sha256");
    source.insert("canonical_id".into(), json!("legacy-raw-canonical-id"));
    QualificationSummaryV1::from_json(legacy)
        .expect_err("resolved receipt identities no longer accept a raw canonical_id field");

    serde_json::from_value::<ExactSymbolSelectorV1>(
        json!({"kind":"canonical_id","canonical_id":"frozen-oracle-value"}),
    )
    .expect("frozen oracle selectors continue to carry raw canonical IDs");
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
fn task10a_refreeze_binds_methodology_thresholds_and_future_corpus() {
    let methodology = include_bytes!("../../../benchmarks/proof-availability/methodology.md");
    let threshold_bytes =
        include_bytes!("../../../benchmarks/proof-availability/thresholds-v1.json");
    let thresholds = ThresholdsV1::from_json(serde_json::from_slice(threshold_bytes).unwrap())
        .expect("checked thresholds");
    let methodology_sha = format!("{:x}", Sha256::digest(methodology));
    assert_eq!(methodology_sha, thresholds.methodology_sha256);
    assert_eq!(
        methodology_sha,
        "28f11893fc1d0c17c1b1b70aeda74818a311009e24b85d899b2d52fa6c8e0dcf"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(threshold_bytes)),
        "feb145cd778ecd0a1e06e90f24a2dd9e4c2e44a0905364439698dbd5622e246f"
    );
    assert_eq!(
        canonical_thresholds_sha256(&thresholds).unwrap(),
        "c10242b9bd3d288070a50493af890ec9180cab3f16bb0df7762a7f6db5f74bca"
    );

    let mut future = corpus();
    future["thresholds_sha256"] = json!(canonical_thresholds_sha256(&thresholds).unwrap());
    future["methodology_sha256"] = json!(methodology_sha);
    let future = CorpusV1::from_json(future).unwrap();
    future.validate_against_thresholds(&thresholds).unwrap();

    let mut changed_methodology = methodology.to_vec();
    changed_methodology.push(b'\n');
    let mut changed = thresholds.clone();
    changed.methodology_sha256 = format!("{:x}", Sha256::digest(&changed_methodology));
    assert_ne!(
        canonical_thresholds_sha256(&thresholds).unwrap(),
        canonical_thresholds_sha256(&changed).unwrap()
    );
    assert!(future.validate_against_thresholds(&changed).is_err());

    let checked_corpus = CorpusV1::from_json(
        serde_json::from_slice(include_bytes!(
            "../../../benchmarks/proof-availability/corpus-v1.json"
        ))
        .expect("checked corpus JSON"),
    )
    .expect("checked corpus");
    checked_corpus
        .validate_against_thresholds(&thresholds)
        .expect("checked corpus binds refrozen thresholds");
    assert_eq!(
        canonical_corpus_sha256(&checked_corpus).expect("canonical corpus identity"),
        "5a507490554ce4bf9ebe37d380906885feca84bbe07cbb4be5519a1d752ddf31"
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

    let mut rounded_down = cohort_path_file("codestory-rust");
    rounded_down["paths"][0]["oracle_steps"][0]["callsite_expression"]["file_byte_length"] =
        json!(9_007_199_254_740_992u64);
    rounded_down["paths"][0]["oracle_steps"][0]["receipt_line_window"]["file_byte_length"] =
        json!(9_007_199_254_740_992u64);
    let rounded_down = CohortPathFileV1::from_json(rounded_down).expect("safe-integer path file");
    let mut rounded_from_unsafe = cohort_path_file("codestory-rust");
    rounded_from_unsafe["paths"][0]["oracle_steps"][0]["callsite_expression"]["file_byte_length"] =
        json!(9_007_199_254_740_993u64);
    rounded_from_unsafe["paths"][0]["oracle_steps"][0]["receipt_line_window"]["file_byte_length"] =
        json!(9_007_199_254_740_993u64);
    let rounded_from_unsafe =
        CohortPathFileV1::from_json(rounded_from_unsafe).expect("unsafe-integer path file");
    assert_eq!(
        canonical_cohort_path_file_sha256(&rounded_down).expect("safe-integer digest"),
        canonical_cohort_path_file_sha256(&rounded_from_unsafe).expect("rounded digest"),
        "artifact identity must use the same RFC 8785 number semantics as the sealed seam"
    );
}

#[test]
fn cohort_path_file_identity_is_order_stable_and_semantically_sensitive() {
    let path_file =
        CohortPathFileV1::from_json(cohort_path_file("codestory-rust")).expect("path file");
    let baseline = canonical_cohort_path_file_sha256(&path_file).unwrap();
    let reparsed: CohortPathFileV1 = serde_json::from_str(
        &serde_json::to_string_pretty(&serde_json::to_value(&path_file).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        baseline,
        canonical_cohort_path_file_sha256(&reparsed).unwrap()
    );
    let mut changed = path_file;
    changed.paths[0].audit.curator = "another-curator@example.invalid".into();
    assert_ne!(
        baseline,
        canonical_cohort_path_file_sha256(&changed).unwrap()
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
    altered_corpus["cohorts"][0]["path_file_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    let altered_corpus = CorpusV1::from_json(altered_corpus).expect("altered valid corpus");
    QualificationSummaryV1::from_json(report())
        .expect("summary")
        .validate_against_inputs(&altered_corpus, &threshold)
        .expect_err("summary corpus hash must bind exact corpus semantics");
}

#[test]
fn invalid_case_retains_private_payload_but_renders_only_the_safe_umbrella() {
    let threshold = ThresholdsV1::from_json(thresholds()).expect("thresholds");
    let frozen_corpus = CorpusV1::from_json(corpus()).expect("corpus");
    let mut invalid = report();
    invalid["cases"][0]["negative_mutations"] = json!([]);
    rebind_results_digest(&mut invalid);
    let summary: QualificationSummaryV1 = serde_json::from_value(invalid).expect("shape");
    let error = summary
        .validate_against_inputs(&frozen_corpus, &threshold)
        .expect_err("negative-mutation cardinality must remain invalid");
    let failure = error
        .downcast_ref::<CaseValidationFailure>()
        .expect("private invalid-case payload");
    assert_eq!(failure.case_ordinal, 0);
    assert_eq!(failure.case.case_id, "codestory-rust-l1-0");
    assert_eq!(failure.case.repository_id, "codestory-rust");
    assert_eq!(failure.to_string(), "proof_availability_case_invalid");
    assert_eq!(format!("{failure:?}"), "proof_availability_case_invalid");
    assert_eq!(error.to_string(), "proof_availability_case_invalid");
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
    rebind_results_digest(&mut independent);
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
        .validate_against_oracle(
            &CorpusV1::from_json(corpus()).expect("frozen corpus"),
            &parsed_path_files(),
        )
        .expect("report binds all corpus evidence");
    let mut wrong_mutation_binding = report();
    wrong_mutation_binding["cases"][0]["negative_mutations"][0]["mutated_spec"]["steps"][0]["target"] =
        selector("wrong::target", "src/area0/file0.rs");
    rebind_results_digest(&mut wrong_mutation_binding);
    QualificationSummaryV1::from_json(wrong_mutation_binding)
        .expect("summary retains mutation evidence before corpus binding")
        .validate_against_oracle(
            &CorpusV1::from_json(corpus()).expect("frozen corpus"),
            &parsed_path_files(),
        )
        .expect_err("corpus binding rejects altered mutation evidence");
    let mut wrong_contract_digest = report();
    wrong_contract_digest["cases"][0]["product_disposition"]["actual"]["contract_digest"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    rebind_results_digest(&mut wrong_contract_digest);
    QualificationSummaryV1::from_json(wrong_contract_digest)
        .expect("wrong digest remains structurally measurable")
        .validate_against_oracle(
            &CorpusV1::from_json(corpus()).expect("frozen corpus"),
            &parsed_path_files(),
        )
        .expect_err("wrong well-formed digest must fail before evaluation");
    let mut wrong_corpus_hash = report();
    wrong_corpus_hash["provenance"]["corpus_sha256"] = json!(SHA);
    wrong_corpus_hash["environment"]["invocation"]["corpus_sha256"] = json!(SHA);
    rebind_results_digest(&mut wrong_corpus_hash);
    QualificationSummaryV1::from_json(wrong_corpus_hash)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("provenance binds the supplied corpus bytes");
    let mut wrong_threshold_hash = report();
    wrong_threshold_hash["provenance"]["thresholds_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    wrong_threshold_hash["environment"]["invocation"]["thresholds_sha256"] =
        wrong_threshold_hash["provenance"]["thresholds_sha256"].clone();
    rebind_results_digest(&mut wrong_threshold_hash);
    QualificationSummaryV1::from_json(wrong_threshold_hash)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("provenance binds the corpus-frozen threshold identity");
    let mut wrong_project = report();
    wrong_project["environment"]["projects"][0]["source_head"] =
        json!("cccccccccccccccccccccccccccccccccccccccc");
    rebind_results_digest(&mut wrong_project);
    QualificationSummaryV1::from_json(wrong_project)
        .expect("shape remains valid")
        .validate_against_corpus(&CorpusV1::from_json(corpus()).expect("frozen corpus"))
        .expect_err("project materialization binds cohort source identity");
    let mut wrong_binary = report();
    wrong_binary["environment"]["binary_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    QualificationSummaryV1::from_json(wrong_binary)
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
    rebind_results_digest(&mut hard_failure);
    QualificationSummaryV1::from_json(hard_failure).expect("failure evidence is representable");
}

#[test]
fn closed_contracts_reject_hostile_nested_shapes() {
    let mut too_many_steps = cohort_path_file("codestory-rust");
    let extra = too_many_steps["paths"][0]["spec"]["steps"][0].clone();
    while too_many_steps["paths"][0]["spec"]["steps"]
        .as_array()
        .unwrap()
        .len()
        < 7
    {
        too_many_steps["paths"][0]["spec"]["steps"]
            .as_array_mut()
            .unwrap()
            .push(extra.clone());
    }
    assert!(CohortPathFileV1::from_json(too_many_steps).is_err());

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

    assert!(
        serde_json::from_value::<ActivationDecisionV1>(json!({
            "outcome":"delay_full_v3_cut","automatic_thresholds_met":null,"failed_gates":[]
        }))
        .is_err(),
        "Outcome D is a Q1 blocker, not a Q2 decision variant"
    );

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
fn summary_rejects_inventory_and_length_one_trail_equation_violations() {
    let mut effective_mismatch = report();
    effective_mismatch["inventory"][0]["effective_endpoint_rows"] = json!("9");
    rebind_results_digest(&mut effective_mismatch);
    QualificationSummaryV1::from_json(effective_mismatch)
        .expect_err("effective endpoint rows must equal stored CALL rows");

    let mut partition_mismatch = report();
    partition_mismatch["inventory"][0]["unresolved_placeholder_rows"] = json!("1");
    rebind_results_digest(&mut partition_mismatch);
    QualificationSummaryV1::from_json(partition_mismatch)
        .expect_err("exact and unresolved rows must partition stored CALL rows");

    let mut admitted_above_exact = report();
    admitted_above_exact["inventory"][0]["admitted_rows"] = json!("9");
    rebind_results_digest(&mut admitted_above_exact);
    QualificationSummaryV1::from_json(admitted_above_exact)
        .expect_err("admitted rows cannot exceed exact resolved rows");

    let mut length_one_mismatch = report();
    length_one_mismatch["trails"][0]["lengths"][0]["strictly_admitted"] = json!("6");
    rebind_results_digest(&mut length_one_mismatch);
    QualificationSummaryV1::from_json(length_one_mismatch)
        .expect_err("length-one trails must equal the inventory relation counts");

    let mut overflowing_partition = report();
    overflowing_partition["inventory"][0]["stored_call_rows"] = json!(u128::MAX.to_string());
    overflowing_partition["inventory"][0]["effective_endpoint_rows"] = json!(u128::MAX.to_string());
    overflowing_partition["inventory"][0]["exact_resolved_rows"] = json!(u128::MAX.to_string());
    overflowing_partition["inventory"][0]["admitted_rows"] = json!("7");
    overflowing_partition["inventory"][0]["unresolved_placeholder_rows"] = json!("1");
    rebind_results_digest(&mut overflowing_partition);
    QualificationSummaryV1::from_json(overflowing_partition)
        .expect_err("inventory partition arithmetic must fail closed on overflow");
}

#[test]
fn finalization_tool_failure_retains_trace_without_claiming_receipts() {
    for failure in ["receipt_integration", "projection_budget"] {
        let mut value = report();
        set_tool_failure_case(&mut value["cases"][0], failure);
        rebind_results_digest(&mut value);

        let parsed = QualificationSummaryV1::from_json(value)
            .expect("tool failure remains a valid immutable case row");
        assert!(matches!(
            parsed.cases[0].product_disposition.actual,
            ActualProductResultV1::Invalid { .. }
        ));
        assert!(!parsed.cases[0].proof_trace.steps.is_empty());
        assert!(
            parsed.cases[0]
                .receipt_evidence
                .observed_receipts
                .is_empty()
        );
        assert_eq!(parsed.cases[0].negative_mutations.len(), 2);
    }
}

fn set_tool_failure_case(case: &mut Value, failure: &str) {
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
    case["product_disposition"] = json!({
        "kind":"invalid",
        "gaps":[],
        "authoritative_receipts":[],
        "actual":{"kind":"invalid","failure":{"stage":"tool_execution","code":"internal"}}
    });
    case["actionable_exact_gap"] = Value::Null;
    case["receipt_evidence"]["observed_receipts"] = json!([]);
    case["receipt_evidence"]["missing_oracle_steps"] = Value::Array(missing);
    case["proof_trace"]["finalization"] = json!({"kind":"failed","failure":failure});
    case["complete_projection_bytes"] = json!(0);
    case["transport"] = json!({
        "kind":"error",
        "error":{"kind":"invalid_projection","projection":"product_tool_failure"}
    });
}

fn set_receipt_budget_case(case: &mut Value) {
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
    case["product_disposition"] = json!({
        "kind":"unknown",
        "gaps":["projection_budget"],
        "authoritative_receipts":[],
        "actual":{
            "kind":"unknown",
            "contract_digest":SHA,
            "gaps":[{"kind":"output_budget_exceeded"}],
            "connected_receipts":[]
        }
    });
    case["actionable_exact_gap"] = json!({
        "gap":{"kind":"output_budget_exceeded"},
        "boundary":{"kind":"finalization","after_step_count":case["attempted_step_count"]}
    });
    case["receipt_evidence"]["missing_oracle_steps"] = Value::Array(missing);
    case["proof_trace"]["finalization"] = json!({"kind":"failed","failure":"receipt_budget"});
    case["complete_projection_bytes"] = json!(128);
}

fn assert_rebound_report_rejected(mut value: Value, expectation: &str) {
    rebind_results_digest(&mut value);
    QualificationSummaryV1::from_json(value).expect_err(expectation);
}

#[test]
fn finalization_pairings_form_a_closed_state_matrix() {
    QualificationSummaryV1::from_json(report()).expect("complete successful result");

    for failure in ["receipt_integration", "projection_budget"] {
        let mut value = report();
        set_tool_failure_case(&mut value["cases"][0], failure);
        rebind_results_digest(&mut value);
        QualificationSummaryV1::from_json(value).expect("finalization tool failure pairing");
    }

    let mut receipt_budget = report();
    set_receipt_budget_case(&mut receipt_budget["cases"][0]);
    rebind_results_digest(&mut receipt_budget);
    QualificationSummaryV1::from_json(receipt_budget)
        .expect("successful output-budget fallback pairing");

    for error in [
        json!({"kind":"serialization","message":"fallback encode failed"}),
        json!({"kind":"invalid_projection","projection":"fallback"}),
        json!({"kind":"output_schema_violation"}),
        json!({"kind":"result_exceeds_budget","maximum_bytes":65536,"actual_bytes":65537}),
    ] {
        let mut fallback_transport_error = report();
        set_receipt_budget_case(&mut fallback_transport_error["cases"][0]);
        fallback_transport_error["cases"][0]["transport"] = json!({"kind":"error","error":error});
        rebind_results_digest(&mut fallback_transport_error);
        QualificationSummaryV1::from_json(fallback_transport_error)
            .expect("Task 4 transport errors remain exact fallback evidence");
    }
}

#[test]
fn finalization_pairings_reject_crossed_or_incomplete_states() {
    for failure in ["receipt_integration", "receipt_budget", "projection_budget"] {
        let mut proven_failed = report();
        proven_failed["cases"][0]["proof_trace"]["finalization"] =
            json!({"kind":"failed","failure":failure});
        proven_failed["cases"][0]["complete_projection_bytes"] = json!(0);
        assert_rebound_report_rejected(
            proven_failed,
            "ContractProven and receipts cannot pair with failed finalization",
        );
    }

    let mut invalid_receipt_budget = report();
    set_tool_failure_case(&mut invalid_receipt_budget["cases"][0], "receipt_budget");
    assert_rebound_report_rejected(
        invalid_receipt_budget,
        "receipt budget is a successful fallback, not a tool failure",
    );

    let mut tool_failure_with_receipt = report();
    let observed =
        tool_failure_with_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0].clone();
    set_tool_failure_case(
        &mut tool_failure_with_receipt["cases"][0],
        "receipt_integration",
    );
    tool_failure_with_receipt["cases"][0]["receipt_evidence"]["observed_receipts"] =
        json!([observed]);
    assert_rebound_report_rejected(
        tool_failure_with_receipt,
        "tool failure cannot retain a receipt comparison",
    );

    for failure in ["receipt_integration", "projection_budget"] {
        let mut fallback_as_tool_failure = report();
        set_receipt_budget_case(&mut fallback_as_tool_failure["cases"][0]);
        fallback_as_tool_failure["cases"][0]["proof_trace"]["finalization"] =
            json!({"kind":"failed","failure":failure});
        fallback_as_tool_failure["cases"][0]["complete_projection_bytes"] = json!(0);
        assert_rebound_report_rejected(
            fallback_as_tool_failure,
            "tool failures cannot carry a successful budget fallback",
        );
    }

    let mut zero_fallback = report();
    set_receipt_budget_case(&mut zero_fallback["cases"][0]);
    zero_fallback["cases"][0]["complete_projection_bytes"] = json!(0);
    assert_rebound_report_rejected(zero_fallback, "budget fallback bytes are exact and nonzero");

    let mut fallback_receipt_subset = report();
    set_receipt_budget_case(&mut fallback_receipt_subset["cases"][0]);
    let observed = &fallback_receipt_subset["cases"][0]["receipt_evidence"]["observed_receipts"][0];
    let receipt_id = observed["receipt_id"].clone();
    let edge_id = observed["edge_id"].clone();
    let projected_edge_id = edge_id.as_i64().expect("fixture edge id").to_string();
    fallback_receipt_subset["cases"][0]["product_disposition"]["authoritative_receipts"] =
        json!([{"receipt_id":receipt_id,"edge_id":edge_id}]);
    fallback_receipt_subset["cases"][0]["product_disposition"]["actual"]["connected_receipts"] =
        json!([{"receipt_id":receipt_id,"edge_id":projected_edge_id}]);
    assert_rebound_report_rejected(
        fallback_receipt_subset,
        "budget fallback cannot claim an authoritative receipt subset",
    );

    let mut wrong_budget_gap = report();
    set_receipt_budget_case(&mut wrong_budget_gap["cases"][0]);
    wrong_budget_gap["cases"][0]["product_disposition"]["actual"]["gaps"] =
        json!([{"kind":"direct_call_missing","step_index":0}]);
    wrong_budget_gap["cases"][0]["product_disposition"]["gaps"] = json!(["relation_missing"]);
    wrong_budget_gap["cases"][0]["actionable_exact_gap"] = json!({
        "gap":{"kind":"direct_call_missing","step_index":0},
        "boundary":{"kind":"step","step_index":0}
    });
    assert_rebound_report_rejected(
        wrong_budget_gap,
        "receipt budget requires the output-budget gap",
    );

    let mut fallback_as_product_failure = report();
    set_receipt_budget_case(&mut fallback_as_product_failure["cases"][0]);
    fallback_as_product_failure["cases"][0]["transport"] = json!({
        "kind":"error",
        "error":{"kind":"invalid_projection","projection":"product_tool_failure"}
    });
    assert_rebound_report_rejected(
        fallback_as_product_failure,
        "successful fallback cannot use the tool-failure transport sentinel",
    );

    let mut zero_complete = report();
    zero_complete["cases"][0]["proof_trace"]["finalization"] =
        json!({"kind":"complete","projection_bytes":0});
    zero_complete["cases"][0]["complete_projection_bytes"] = json!(0);
    assert_rebound_report_rejected(zero_complete, "complete projection bytes are nonzero");

    let mut fallback_marked_complete = report();
    set_receipt_budget_case(&mut fallback_marked_complete["cases"][0]);
    fallback_marked_complete["cases"][0]["proof_trace"]["finalization"] =
        json!({"kind":"complete","projection_bytes":128});
    assert_rebound_report_rejected(
        fallback_marked_complete,
        "output-budget fallback cannot be marked complete",
    );

    let mut tool_failure_marked_complete = report();
    let complete_transport = tool_failure_marked_complete["cases"][0]["transport"].clone();
    set_tool_failure_case(
        &mut tool_failure_marked_complete["cases"][0],
        "projection_budget",
    );
    tool_failure_marked_complete["cases"][0]["proof_trace"]["finalization"] =
        json!({"kind":"complete","projection_bytes":128});
    tool_failure_marked_complete["cases"][0]["complete_projection_bytes"] = json!(128);
    tool_failure_marked_complete["cases"][0]["transport"] = complete_transport;
    assert_rebound_report_rejected(
        tool_failure_marked_complete,
        "Complete requires a successful product result",
    );
}

#[test]
fn producer_mapping_red_requires_ordered_oracles_and_lossless_task4_errors() {
    let mut wrong_target_order = cohort_path_file("codestory-rust");
    wrong_target_order["paths"][0]["spec"]["steps"][0]["target"] =
        selector("crate::wrong_target", "src/area0/file0.rs");
    assert!(CohortPathFileV1::from_json(wrong_target_order).is_err());

    let mut task4_serialization = report();
    task4_serialization["cases"][0]["transport"] = json!({
        "kind":"error",
        "error":{"kind":"serialization","message":"exact encoder failure"}
    });
    rebind_results_digest(&mut task4_serialization);
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
        rebind_results_digest(&mut value);
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

    let mut wrong_chain = cohort_path_file("codestory-rust");
    wrong_chain["paths"][10]["oracle_steps"][1]["caller"]["symbol"] = json!("broken");
    wrong_chain["paths"][10]["oracle_steps"][1]["caller"]["selector"] =
        selector("broken", "src/area0/file0.rs");
    assert!(CohortPathFileV1::from_json(wrong_chain).is_err());
    let mut wrong_target_count = cohort_path_file("codestory-rust");
    wrong_target_count["paths"][0]["spec"]["steps"]
        .as_array_mut()
        .unwrap()
        .push(json!({"target":selector("extra::target", "src/area0/file0.rs")}));
    assert!(CohortPathFileV1::from_json(wrong_target_count).is_err());
    let mut wrong_mutation = cohort_path_file("codestory-rust");
    wrong_mutation["paths"][0]["negative_mutations"][0]["path_id"] = json!("other");
    assert!(CohortPathFileV1::from_json(wrong_mutation).is_err());
    let mut wrong_range = cohort_path_file("codestory-rust");
    wrong_range["paths"][0]["oracle_steps"][0]["receipt_line_window"]["end_byte"] = json!(4097);
    assert!(CohortPathFileV1::from_json(wrong_range).is_err());
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
        project_file_components: vec!["src".into(), "area0".into(), "file0.rs".into()],
    };
    let task6_receipt = IndexedCallEdgeReceipt {
        receipt: ReceiptRef {
            receipt_id: "indexed-call-edge:fixture".into(),
            edge_id: "-42".into(),
        },
        source: identity("-1", "codestory-rust-l1-0::start"),
        target: identity("-2", "codestory-rust-l1-0::target_0"),
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
            project_file_components: vec!["src".into(), "area0".into(), "file0.rs".into()],
            indexed_sha256: SHA.into(),
            observed_sha256: SHA.into(),
            anchor_line: 1,
            byte_start: 11,
            byte_end: 19,
            text: "call();\n".into(),
        },
    };
    let path_file =
        CohortPathFileV1::from_json(cohort_path_file("codestory-rust")).expect("oracle path file");
    let oracle_path = &path_file.paths[0];
    let positive_input = contracts::oracle_path_product_contract(oracle_path)
        .expect("positive product contract conversion");
    assert!(matches!(
        codestory_agent::proof_qualification_support::validate_contract(positive_input).unwrap(),
        codestory_agent::proof_qualification_support::ValidationOutcome::Validated { .. }
    ));
    let mutation_id = &oracle_path.negative_mutations[0].mutation_id;
    let negative_input = contracts::negative_mutation_product_contract(oracle_path, mutation_id)
        .expect("negative product contract conversion");
    let frozen_hashes =
        match codestory_agent::proof_qualification_support::validate_contract(negative_input)
            .unwrap()
        {
            codestory_agent::proof_qualification_support::ValidationOutcome::Validated {
                hashes,
                ..
            } => hashes,
            other => panic!("expected validated frozen negative mutation, got {other:?}"),
        };
    let mut caller_altered_row = oracle_path.negative_mutations[0].clone();
    caller_altered_row.mutated_spec = oracle_path.spec.clone();
    let resolved_by_id =
        contracts::negative_mutation_product_contract(oracle_path, &caller_altered_row.mutation_id)
            .expect("same ID resolves the frozen row, not the caller copy");
    let resolved_hashes =
        match codestory_agent::proof_qualification_support::validate_contract(resolved_by_id)
            .unwrap()
        {
            codestory_agent::proof_qualification_support::ValidationOutcome::Validated {
                hashes,
                ..
            } => hashes,
            other => panic!("expected validated ID-resolved negative mutation, got {other:?}"),
        };
    assert_eq!(
        frozen_hashes.contract_digest(),
        resolved_hashes.contract_digest(),
        "a same-ID caller object with an altered spec cannot alter the executed frozen row"
    );
    let derived_comparison =
        contracts::compare_task6_receipt_to_oracle(0, &task6_receipt, &oracle_path.oracle_steps[0])
            .expect("receipt comparison");
    assert!(matches!(
        derived_comparison,
        ReceiptOracleComparisonV1::Exact { .. }
    ));
    let derived_observed =
        contracts::observed_receipt_from_task6(0, &task6_receipt, &oracle_path.oracle_steps[0])
            .expect("receipt conversion derives its oracle comparison");
    assert_eq!(derived_observed.edge_id, -42);
    let mut mismatched_receipt = task6_receipt.clone();
    mismatched_receipt.source.qualified_name = "wrong::caller".into();
    mismatched_receipt.line_window.anchor_line = 2;
    mismatched_receipt.line_window.byte_start = 12;
    mismatched_receipt.target.qualified_name = "wrong::target".into();
    assert_eq!(
        contracts::compare_task6_receipt_to_oracle(
            0,
            &mismatched_receipt,
            &oracle_path.oracle_steps[0],
        )
        .unwrap(),
        ReceiptOracleComparisonV1::Mismatched {
            oracle_step_index: 0,
            oracle_step: contracts::ReceiptOracleStepV1::from(&oracle_path.oracle_steps[0]),
            mismatches: vec![
                contracts::ReceiptMismatchFieldV1::Caller,
                contracts::ReceiptMismatchFieldV1::CallsiteLine,
                contracts::ReceiptMismatchFieldV1::CallsiteWindow,
                contracts::ReceiptMismatchFieldV1::Target,
            ],
        }
    );
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
    assert_eq!(
        observed.source.canonical_id_binding_sha256,
        contracts::resolved_canonical_id_binding_sha256(&observed.source.pinned, "canonical--1")
            .unwrap()
    );
    assert_eq!(observed.source.qualified_name, "codestory-rust-l1-0::start");
    assert_eq!(
        observed.source.project_file_components,
        ["src", "area0", "file0.rs"]
    );
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
    assert_eq!(
        observed.target.canonical_id_binding_sha256,
        contracts::resolved_canonical_id_binding_sha256(&observed.target.pinned, "canonical--2")
            .unwrap()
    );
    assert_eq!(
        observed.target.qualified_name,
        "codestory-rust-l1-0::target_0"
    );
    assert_eq!(
        observed.target.project_file_components,
        ["src", "area0", "file0.rs"]
    );

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

#[test]
fn admitted_callsite_identity_is_opaque_after_task6() {
    use codestory_agent::proof_qualification_support::{
        CallableContainmentEvidence, IndexedCallEdgeReceipt, IndexedLineWindow, PinnedNodeIdentity,
        ReceiptRef, ResolvedNodeIdentity,
    };
    use codestory_contracts::graph::{NodeId, ResolutionCertainty};

    const RAW_TARGET: i64 = -8_657_445_931_347_514_024;
    const RESOLVED_TARGET: i64 = -8_657_442_632_812_629_391;

    let path_file =
        CohortPathFileV1::from_json(cohort_path_file("codestory-rust")).expect("oracle path file");
    let oracle = &path_file.paths[0].oracle_steps[0];
    let project_file_components = oracle
        .receipt_line_window
        .path
        .split('/')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let identity =
        |node_id: i64, declaration: &contracts::OracleDeclarationV1| ResolvedNodeIdentity {
            pinned: PinnedNodeIdentity {
                project_id: "project".into(),
                core_generation_id: "generation".into(),
                core_run_id: "run".into(),
                node_id: node_id.to_string(),
            },
            canonical_id: format!("canonical-{node_id}"),
            qualified_name: declaration.symbol.clone(),
            project_file_components: project_file_components.clone(),
        };
    let callsite_identity = format!("-3:{}:0:{RAW_TARGET}|fixture", oracle.callsite_line);
    let receipt = IndexedCallEdgeReceipt {
        receipt: ReceiptRef {
            receipt_id: "indexed-call-edge:raw-resolved-split".into(),
            edge_id: "-42".into(),
        },
        source: identity(-1, &oracle.caller),
        target: identity(RESOLVED_TARGET, &oracle.target),
        certainty: ResolutionCertainty::Certain,
        callsite_identity: callsite_identity.clone(),
        containment: CallableContainmentEvidence {
            file_node_id: NodeId(-3),
            owner_node_id: NodeId(-1),
            start_line: oracle.callsite_line,
            end_line: oracle.callsite_line,
        },
        line_window: IndexedLineWindow {
            kind: "indexed_line_v1",
            project_file_components,
            indexed_sha256: oracle.receipt_file_sha256.clone(),
            observed_sha256: oracle.receipt_file_sha256.clone(),
            anchor_line: oracle.callsite_line,
            byte_start: usize::try_from(oracle.receipt_line_window.start_byte).unwrap(),
            byte_end: usize::try_from(oracle.receipt_line_window.end_byte).unwrap(),
            text: "call();\n".into(),
        },
    };

    let observed = contracts::observed_receipt_from_task6(0, &receipt, oracle)
        .expect("Task 6 already admitted the raw call occurrence");
    assert_eq!(observed.callsite_identity, callsite_identity);
    assert_eq!(observed.target.pinned.node_id, RESOLVED_TARGET.to_string());

    let mut closed = report();
    closed["cases"][0]["receipt_evidence"]["observed_receipts"][0]["callsite_identity"] =
        json!(format!("-10000:1:0:{RAW_TARGET}|fixture"));
    closed["cases"][0]["receipt_evidence"]["observed_receipts"][0]["target"]["pinned"]["node_id"] =
        json!(RESOLVED_TARGET.to_string());
    closed["cases"][0]["proof_trace"]["selectors"][1]["outcome"]["node_id"] =
        json!(RESOLVED_TARGET);
    rebind_results_digest(&mut closed);

    let mut empty_identity = closed.clone();
    empty_identity["cases"][0]["receipt_evidence"]["observed_receipts"][0]["callsite_identity"] =
        json!("");
    rebind_results_digest(&mut empty_identity);
    QualificationSummaryV1::from_json(empty_identity)
        .expect_err("the opaque diagnostic identity remains non-empty");

    let parsed = QualificationSummaryV1::from_json(closed)
        .expect("closed report validation keeps the admitted identity opaque");
    parsed
        .validate_against_oracle(
            &CorpusV1::from_json(corpus()).expect("corpus"),
            &parsed_path_files(),
        )
        .expect("the raw and resolved targets retain the same frozen oracle relation");
}

fn assert_valid_first_zero(gate: &str, kind: &str, reason: &str) {
    let mut value = report();
    let case = &mut value["cases"][0];
    let observed = case["receipt_evidence"]["observed_receipts"]
        .as_array_mut()
        .expect("observed receipts")
        .remove(0);
    let (coarse_gap, actual_gap) = match (gate, reason) {
        ("raw_admission", _) => (
            Some("relation_missing"),
            Some(json!({"kind":"direct_call_missing","step_index":0})),
        ),
        ("containment", _) => (
            Some("source_binding"),
            Some(json!({"kind":"edge_containment_unproven","step_index":0})),
        ),
        ("source_binding", "invalid_utf8") => (
            Some("source_binding"),
            Some(json!({"kind":"invalid_utf8","step_index":0})),
        ),
        ("source_binding", _) => (None, None),
        ("line", "line_missing") => (
            Some("source_binding"),
            Some(json!({"kind":"source_line_out_of_range","step_index":0})),
        ),
        ("line", "line_over_limit") => (
            Some("source_binding"),
            Some(json!({"kind":"source_window_too_large","step_index":0})),
        ),
        _ => panic!("unsupported trace cause {gate}/{reason}"),
    };
    case["product_disposition"] = actual_gap.as_ref().map_or_else(
        || json!({
            "kind":"unknown","gaps":[],"authoritative_receipts":[],
            "actual":{"kind":"unavailable","contract_digest":SHA,"reasons":["source_not_bound_to_publication"]}
        }),
        |gap| json!({
            "kind":"unknown","gaps":[coarse_gap.unwrap()],"authoritative_receipts":[],
            "actual":{"kind":"unknown","contract_digest":SHA,"gaps":[gap]}
        }),
    );
    case["actionable_exact_gap"] = actual_gap.map_or(Value::Null, |gap| {
        json!({
            "gap":gap,
            "boundary":{"kind":"step","step_index":0}
        })
    });
    case["receipt_evidence"]["missing_oracle_steps"] = json!([{
        "step_index":0,"oracle_step":observed["oracle_comparison"]["oracle_step"]
    }]);
    case["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor",
        "gate":gate,
        "histogram":[{"reason":{"kind":kind,"reason":reason},"edge_ids":[1]}]
    });
    rebuild_funnel(&mut value);
    rebind_results_digest(&mut value);
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
        "kind":match disposition { "unavailable" => "unknown", other => other },
        "gaps":gap.into_iter().collect::<Vec<_>>(),
        "authoritative_receipts":[],
        "actual":match disposition {
            "unknown" => json!({"kind":"unknown","contract_digest":SHA,"gaps":[match gap.unwrap_or("selector_missing") {
                "selector_missing" => json!({"kind":"selector_missing","selector_index":0}),
                "selector_ambiguous" => json!({"kind":"selector_ambiguous","selector_index":0}),
                "relation_missing" => json!({"kind":"direct_call_missing","step_index":0}),
                "recursion" => json!({"kind":"recursive_call_not_representable","step_index":0}),
                "source_binding" => json!({"kind":"source_window_too_large","step_index":0}),
                "projection_budget" => json!({"kind":"output_budget_exceeded"}),
                other => panic!("unsupported fixture gap {other}"),
            }]}),
            "certified_absence" => json!({"kind":"contract_refuted","contract_digest":SHA,"basis":{"kind":"certified_absence","step_index":0,"extractor_capability_receipt_id":"extractor:fixture","enumeration_receipt_id":"enumeration:fixture"}}),
            "unavailable" => json!({"kind":"unavailable","contract_digest":SHA,"reasons":["source_not_bound_to_publication"]}),
            "invalid" => json!({"kind":"invalid","failure":{"stage":"tool_execution","code":"invalid_fixture"}}),
            other => panic!("unsupported fixture disposition {other}"),
        }
    });
    case["actionable_exact_gap"] = gap
        .map(|gap| match gap {
            "selector_missing" => json!({
                "gap":{"kind":"selector_missing","selector_index":0},
                "boundary":{"kind":"selector","selector_index":0}
            }),
            "selector_ambiguous" => json!({
                "gap":{"kind":"selector_ambiguous","selector_index":0},
                "boundary":{"kind":"selector","selector_index":0}
            }),
            "relation_missing" => json!({
                "gap":{"kind":"direct_call_missing","step_index":0},
                "boundary":{"kind":"step","step_index":0}
            }),
            "recursion" => json!({
                "gap":{"kind":"recursive_call_not_representable","step_index":0},
                "boundary":{"kind":"step","step_index":0}
            }),
            "source_binding" => json!({
                "gap":{"kind":"source_window_too_large","step_index":0},
                "boundary":{"kind":"step","step_index":0}
            }),
            "projection_budget" => json!({
                "gap":{"kind":"output_budget_exceeded"},
                "boundary":{"kind":"finalization","after_step_count":case["attempted_step_count"]}
            }),
            other => panic!("unsupported fixture actionable gap {other}"),
        })
        .unwrap_or(Value::Null);
    if disposition == "unknown" {
        match gap {
            Some("selector_missing") | Some("selector_ambiguous") => {
                let reason = gap.unwrap().strip_prefix("selector_").unwrap();
                case["proof_trace"]["selectors"][0]["outcome"] =
                    json!({"kind":"failed","reason":reason});
                case["proof_trace"]["selector_early_return"] = json!(true);
                case["proof_trace"]["steps"] = json!([]);
                case["unclassified_step_indices"] = Value::Array(
                    (0..case["attempted_step_count"].as_u64().unwrap())
                        .map(|index| json!(index))
                        .collect(),
                );
            }
            Some("relation_missing") => {
                case["proof_trace"]["steps"][0]["candidate_edge_ids"] = json!([]);
                case["proof_trace"]["steps"][0]["outcome"] = json!({
                    "kind":"first_zero_survivor","gate":"raw_admission","histogram":[]
                });
            }
            Some("recursion") => {
                let source_node = case["proof_trace"]["selectors"][0]["outcome"]["node_id"].clone();
                case["proof_trace"]["selectors"][1]["outcome"]["node_id"] = source_node;
                case["proof_trace"]["steps"][0]["candidate_edge_ids"] = json!([]);
                case["proof_trace"]["steps"][0]["outcome"] = json!({
                    "kind":"first_zero_survivor","gate":"raw_admission","histogram":[]
                });
            }
            Some("source_binding") => {
                case["proof_trace"]["steps"][0]["outcome"] = json!({
                    "kind":"first_zero_survivor","gate":"line",
                    "histogram":[{"reason":{"kind":"source_binding","reason":"line_over_limit"},"edge_ids":[1]}]
                });
            }
            Some("projection_budget") | None => {}
            Some(other) => panic!("unsupported trace fixture gap {other}"),
        }
        if matches!(
            gap,
            Some("selector_missing")
                | Some("selector_ambiguous")
                | Some("relation_missing")
                | Some("recursion")
                | Some("source_binding")
        ) {
            case["receipt_evidence"]["observed_receipts"] = json!([]);
        }
    }
    rebuild_funnel(value);
}

fn six_step_case(value: &mut Value) -> &mut Value {
    value["cases"]
        .as_array_mut()
        .expect("cases")
        .iter_mut()
        .find(|case| case["attempted_step_count"] == 6)
        .expect("six-step fixture case")
}

fn selector_missing_gaps() -> Vec<Value> {
    (0..=6)
        .map(|selector_index| json!({"kind":"selector_missing","selector_index":selector_index}))
        .collect()
}

fn set_six_step_selector_unknown_case(value: &mut Value, matching_trace: bool) {
    let case = six_step_case(value);
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
    case["product_disposition"] = json!({
        "kind":"unknown",
        "gaps":["selector_missing"],
        "authoritative_receipts":[],
        "actual":{
            "kind":"unknown",
            "contract_digest":SHA,
            "gaps":selector_missing_gaps(),
            "connected_receipts":[]
        }
    });
    case["receipt_evidence"]["missing_oracle_steps"] = Value::Array(missing);
    if matching_trace {
        for selector in case["proof_trace"]["selectors"]
            .as_array_mut()
            .expect("selector traces")
        {
            selector["outcome"] = json!({"kind":"failed","reason":"missing"});
        }
        case["proof_trace"]["selector_early_return"] = json!(true);
        case["proof_trace"]["steps"] = json!([]);
        case["unclassified_step_indices"] = json!([0, 1, 2, 3, 4, 5]);
        case["receipt_evidence"]["observed_receipts"] = json!([]);
        case["actionable_exact_gap"] = json!({
            "gap":{"kind":"selector_missing","selector_index":0},
            "boundary":{"kind":"selector","selector_index":0}
        });
    } else {
        case["actionable_exact_gap"] = Value::Null;
    }
    rebuild_funnel(value);
}

#[test]
fn actual_unknown_accepts_all_seven_selector_gaps_when_the_trace_matches() {
    let mut value = report();
    set_six_step_selector_unknown_case(&mut value, true);
    rebind_results_digest(&mut value);

    let parsed = QualificationSummaryV1::from_json(value)
        .expect("seven legal selector gaps must remain structurally reportable");
    let actual = &parsed
        .cases
        .iter()
        .find(|case| case.attempted_step_count == 6)
        .expect("six-step parsed case")
        .product_disposition
        .actual;
    assert!(matches!(
        actual,
        ActualProductResultV1::Unknown { gaps, .. } if gaps.len() == 7
    ));
    assert!(
        parsed
            .cases
            .iter()
            .find(|case| case.attempted_step_count == 6)
            .expect("six-step parsed case")
            .evaluable_facts()
            .expect("evaluable facts")
            .product_disposition_matches_evidence
    );
}

#[test]
fn actual_unknown_trace_mismatch_remains_structurally_reportable_but_fails_evidence_gate() {
    let mut value = report();
    set_six_step_selector_unknown_case(&mut value, false);
    rebind_results_digest(&mut value);

    let parsed = QualificationSummaryV1::from_json(value)
        .expect("trace disagreements must remain measurable evidence");
    assert!(
        !parsed
            .cases
            .iter()
            .find(|case| case.attempted_step_count == 6)
            .expect("six-step parsed case")
            .evaluable_facts()
            .expect("evaluable facts")
            .product_disposition_matches_evidence
    );
}

#[test]
fn actual_unknown_rejects_empty_duplicate_invalid_index_and_digest_gaps() {
    let mut empty = report();
    set_six_step_selector_unknown_case(&mut empty, true);
    six_step_case(&mut empty)["product_disposition"]["actual"]["gaps"] = json!([]);
    assert_rebound_report_rejected(empty, "Unknown requires at least one actual gap");

    let mut duplicate = report();
    set_six_step_selector_unknown_case(&mut duplicate, true);
    let duplicate_gap =
        six_step_case(&mut duplicate)["product_disposition"]["actual"]["gaps"][0].clone();
    six_step_case(&mut duplicate)["product_disposition"]["actual"]["gaps"]
        .as_array_mut()
        .expect("actual gaps")
        .push(duplicate_gap);
    assert_rebound_report_rejected(duplicate, "actual gaps must be unique");

    let mut invalid_selector = report();
    set_six_step_selector_unknown_case(&mut invalid_selector, true);
    six_step_case(&mut invalid_selector)["product_disposition"]["actual"]["gaps"][6] =
        json!({"kind":"selector_missing","selector_index":7});
    assert_rebound_report_rejected(
        invalid_selector,
        "selector index seven is outside the domain",
    );

    let mut invalid_step = report();
    set_six_step_selector_unknown_case(&mut invalid_step, true);
    six_step_case(&mut invalid_step)["product_disposition"]["actual"]["gaps"][6] =
        json!({"kind":"direct_call_missing","step_index":6});
    assert_rebound_report_rejected(invalid_step, "step index six is outside the domain");

    let mut invalid_digest = report();
    set_six_step_selector_unknown_case(&mut invalid_digest, true);
    six_step_case(&mut invalid_digest)["product_disposition"]["actual"]["contract_digest"] =
        json!("not-a-sha256");
    assert_rebound_report_rejected(invalid_digest, "actual results retain their digest binding");
}

#[test]
fn invariant_table_exercises_remaining_closed_decision_and_funnel_variants() {
    for disposition in ["unknown", "certified_absence", "unavailable"] {
        let mut value = report();
        make_non_proven_case(
            &mut value,
            disposition,
            (disposition == "unknown").then_some("selector_missing"),
        );
        rebind_results_digest(&mut value);
        QualificationSummaryV1::from_json(value).expect("closed product disposition");
    }
    let mut invalid = report();
    set_tool_failure_case(&mut invalid["cases"][0], "receipt_integration");
    rebind_results_digest(&mut invalid);
    QualificationSummaryV1::from_json(invalid).expect("closed invalid product disposition");
    let mut unavailable_as_proven = report();
    make_non_proven_case(&mut unavailable_as_proven, "unavailable", None);
    unavailable_as_proven["cases"][0]["product_disposition"]["kind"] = json!("contract_proven");
    rebind_results_digest(&mut unavailable_as_proven);
    QualificationSummaryV1::from_json(unavailable_as_proven)
        .expect_err("Unavailable cannot flatten into ContractProven");
    let mut mismatched_receipt = report();
    make_non_proven_case(&mut mismatched_receipt, "unavailable", None);
    mismatched_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0]["target"]["qualified_name"] =
        json!("crate::false_positive");
    let oracle = mismatched_receipt["cases"][0]["receipt_evidence"]["observed_receipts"]
        [0]["oracle_comparison"]["oracle_step"]
        .clone();
    mismatched_receipt["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"] = json!({
        "kind":"mismatched","oracle_step_index":0,"oracle_step":oracle,
        "mismatches":["target"]
    });
    rebind_results_digest(&mut mismatched_receipt);
    QualificationSummaryV1::from_json(mismatched_receipt)
        .expect("closed mismatched receipt comparison");
    for gap in [
        "selector_missing",
        "selector_ambiguous",
        "relation_missing",
        "recursion",
        "source_binding",
    ] {
        let mut value = report();
        make_non_proven_case(&mut value, "unknown", Some(gap));
        rebind_results_digest(&mut value);
        QualificationSummaryV1::from_json(value).expect("closed actionable gap");
    }
    let mut projection_budget = report();
    set_receipt_budget_case(&mut projection_budget["cases"][0]);
    rebind_results_digest(&mut projection_budget);
    QualificationSummaryV1::from_json(projection_budget)
        .expect("projection budget uses the closed receipt-budget fallback pairing");
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
        contracts::schema_json(SchemaDocument::Path)["properties"]["paths"]["minItems"],
        30
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Thresholds)["$defs"]["RoleThresholdsV1"]["properties"]
            ["minimum_positive_step_recall_milli"]["maximum"],
        1000
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Path)["$defs"]["CallPathSpecV1"]["properties"]["steps"]
            ["maxItems"],
        6
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Path)["$defs"]["ProofContractFieldV1"]["oneOf"][1]["properties"]
            ["step"]["maximum"],
        5
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Path)["$defs"]["ProofContractFieldV1"]["oneOf"][5]["properties"]
            ["index"]["maximum"],
        5
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
    let report_schema = contracts::schema_json(SchemaDocument::Report);
    assert_eq!(
        report_schema["$id"],
        "codestory.proof-availability-report/v2"
    );
    assert_eq!(
        report_schema["$defs"]["ResolvedNodeIdentityV1"]["properties"]["canonical_id_binding_sha256"]
            ["pattern"],
        "^[0-9a-f]{64}$"
    );
    assert!(
        report_schema["$defs"]["ResolvedNodeIdentityV1"]["properties"]
            .get("canonical_id")
            .is_none()
    );
    assert_eq!(
        report_schema["$defs"]["EnvironmentReportV1"]["properties"]["qualification_source_commit"]
            ["pattern"],
        "^[0-9a-f]{40}$"
    );
    assert_eq!(
        report_schema["$defs"]["EnvironmentReportV1"]["properties"]["recorded_at"]["pattern"],
        "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?Z$"
    );
    assert_eq!(
        report_schema["$defs"]["EnvironmentReportV1"]["properties"]["qualification_id"]["pattern"],
        "^[0-9]{8}T[0-9]{6}Z-[0-9a-f]{12}$"
    );
    assert_eq!(
        report_schema["$defs"]["QualificationBuildProvenanceV1"]["properties"]["rustc_vv"]["maxLength"],
        8192
    );
    assert_eq!(
        report_schema["$defs"]["QualificationBuildProvenanceV1"]["properties"]["cargo_profile"]["const"],
        "release"
    );
    assert_eq!(
        report_schema["$defs"]["QualificationBuildProvenanceV1"]["properties"]["prescribed_argv"]["const"],
        json!([
            "cargo",
            "build",
            "--release",
            "--locked",
            "-p",
            "codestory-bench",
            "--bin",
            "codestory-proof-availability"
        ])
    );
    assert_eq!(
        report_schema["$defs"]["QualificationBuildProvenanceV1"]["properties"]["source_commit"]["pattern"],
        "^[0-9a-f]{40}$"
    );
    assert_eq!(
        report_schema["$defs"]["QualificationBuildProvenanceV1"]["properties"]["source_tree"]["pattern"],
        "^[0-9a-f]{40}$"
    );
    assert_eq!(
        report_schema["$defs"]["QualificationBuildProvenanceV1"]["properties"]["source_dirty"]["const"],
        false
    );
    assert_eq!(
        report_schema["$defs"]["ActualProductResultV1"]["oneOf"][0]["properties"]["contract_digest"]
            ["pattern"],
        "^[0-9a-f]{64}$"
    );
    assert_eq!(
        report_schema["$defs"]["ActualProductResultV1"]["oneOf"][0]["properties"]["receipts"]["maxItems"],
        6
    );
    assert_eq!(
        report_schema["$defs"]["ActualProductResultV1"]["oneOf"][2]["properties"]["gaps"]["minItems"],
        1
    );
    assert_eq!(
        report_schema["$defs"]["ActualProductResultV1"]["oneOf"][2]["properties"]["gaps"]["maxItems"],
        76
    );
    assert_eq!(
        report_schema["$defs"]["ProductDispositionV1"]["properties"]["gaps"]["maxItems"],
        6
    );
    assert_eq!(
        report_schema["$defs"]["ActualProductResultV1"]["oneOf"][3]["properties"]["reasons"]["maxItems"],
        4
    );
    assert_eq!(
        report_schema["$defs"]["ProjectedReceiptReferenceV1"]["properties"]["edge_id"]["pattern"],
        "^-?(0|[1-9][0-9]*)$"
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
        contracts::schema_json(SchemaDocument::Path)["$defs"]["OracleStepV1"]["properties"]["receipt_file_sha256"]
            ["pattern"],
        "^[0-9a-f]{64}$"
    );
    assert_eq!(
        contracts::schema_json(SchemaDocument::Report)["$defs"]["ReceiptOracleStepV1"]["properties"]
            ["receipt_file_sha256"]["pattern"],
        "^[0-9a-f]{64}$"
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
    let indexed = cli::Cli::try_parse_from([
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
        "--qualification-id",
        "20260821T123456Z-0123456789ab",
    ])
    .expect("indexed materialization with closed qualification identity");
    let cli::Command::Materialize(indexed) = indexed.command else {
        panic!("materialize command")
    };
    assert_eq!(
        indexed.qualification_id.as_deref(),
        Some("20260821T123456Z-0123456789ab")
    );

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
    for invalid in [
        "20260821T123456Z-0123456789a",
        "20260821T123456Z-0123456789AB",
        "2026-08-21T12:34:56Z-0123456789ab",
        "20260230T123456Z-0123456789ab",
    ] {
        assert!(
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
                "--qualification-id",
                invalid,
            ])
            .is_err(),
            "indexed materialization must reject {invalid}"
        );
    }
    assert!(
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
        ])
        .is_err(),
        "indexed materialization requires a qualification ID"
    );
    assert!(
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
            "--verify-only",
            "--qualification-id",
            "20260821T123456Z-0123456789ab",
        ])
        .is_err(),
        "source-only audit rejects indexed qualification identity"
    );
    assert!(matches!(
        cli::Cli::try_parse_from([
            "bin",
            "run",
            "--corpus",
            "/tmp/c",
            "--thresholds",
            "/tmp/t",
            "--environment",
            "/tmp/e",
            "--out",
            "/tmp/r"
        ])
        .expect("run")
        .command,
        cli::Command::Run(_)
    ));
    assert!(
        cli::Cli::try_parse_from([
            "bin",
            "run",
            "--corpus",
            "/tmp/c",
            "--thresholds",
            "/tmp/t",
            "--environment",
            "/tmp/e",
            "--out",
            "/tmp/r",
            "--source-dependency",
            "/tmp/dependency.json",
        ])
        .is_err(),
        "Q2 rejects caller-supplied outcome-D evidence"
    );
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
            "--corpus",
            "/tmp/c",
            "--thresholds",
            "/tmp/t",
            "--results",
            "/tmp/r",
            "--source-dependency",
            "/tmp/dependency.json",
        ])
        .is_err(),
        "verification has no self-asserted outcome-D input"
    );
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
    for (missing, arguments) in [
        (
            "--corpus",
            vec![
                "bin",
                "run",
                "--thresholds",
                "/tmp/t",
                "--environment",
                "/tmp/e",
                "--out",
                "/tmp/r",
            ],
        ),
        (
            "--thresholds",
            vec![
                "bin",
                "run",
                "--corpus",
                "/tmp/c",
                "--environment",
                "/tmp/e",
                "--out",
                "/tmp/r",
            ],
        ),
        (
            "--environment",
            vec![
                "bin",
                "run",
                "--corpus",
                "/tmp/c",
                "--thresholds",
                "/tmp/t",
                "--out",
                "/tmp/r",
            ],
        ),
        (
            "--out",
            vec![
                "bin",
                "run",
                "--corpus",
                "/tmp/c",
                "--thresholds",
                "/tmp/t",
                "--environment",
                "/tmp/e",
            ],
        ),
    ] {
        assert!(
            cli::Cli::try_parse_from(arguments).is_err(),
            "run must require {missing}"
        );
    }
    assert!(
        cli::Cli::try_parse_from(["bin", "run", "--thresholds", "/tmp/t", "--output", "/tmp/o"])
            .is_err()
    );
}

#[test]
fn run_rejects_thresholds_not_bound_to_the_corpus_before_creating_output() {
    let Some(binary) = option_env!("CARGO_BIN_EXE_codestory-proof-availability") else {
        // This file is also included by the binary's unit-test module to
        // exercise private threshold evaluation. Cargo exposes the built
        // binary only to this integration-test target.
        return;
    };
    let root = tempfile::tempdir().expect("temporary run inputs");
    let repository_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus_path = repository_root.join("benchmarks/proof-availability/corpus-v1.json");
    let mut stale_thresholds: Value = serde_json::from_slice(
        &std::fs::read(repository_root.join("benchmarks/proof-availability/thresholds-v1.json"))
            .expect("checked-in thresholds"),
    )
    .expect("thresholds JSON");
    stale_thresholds["methodology_sha256"] =
        json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    let thresholds_path = root.path().join("stale-thresholds.json");
    std::fs::write(
        &thresholds_path,
        serde_json::to_vec(&stale_thresholds).expect("serialize stale thresholds"),
    )
    .expect("write stale thresholds");
    let output_path = root.path().join("results");

    let output = std::process::Command::new(binary)
        .args([
            "run",
            "--corpus",
            corpus_path.to_str().expect("UTF-8 corpus path"),
            "--thresholds",
            thresholds_path.to_str().expect("UTF-8 thresholds path"),
            "--environment",
            root.path()
                .join("environment.json")
                .to_str()
                .expect("UTF-8 environment path"),
            "--out",
            output_path.to_str().expect("UTF-8 output path"),
        ])
        .output()
        .expect("run proof availability command");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("proof_availability_corpus_threshold_binding_invalid"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_path.exists(), "validation must not create output");
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
    rebind_results_digest(&mut value);

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
    rebind_results_digest(&mut value);

    let mut falsely_authoritative = value.clone();
    let extra =
        falsely_authoritative["cases"][0]["receipt_evidence"]["observed_receipts"][1].clone();
    falsely_authoritative["cases"][0]["product_disposition"]["authoritative_receipts"]
        .as_array_mut()
        .expect("authoritative receipts")
        .push(json!({"receipt_id":extra["receipt_id"],"edge_id":extra["edge_id"]}));
    falsely_authoritative["cases"][0]["product_disposition"]["actual"]["receipts"]
        .as_array_mut()
        .expect("actual product receipts")
        .push(json!({"receipt_id":extra["receipt_id"],"edge_id":extra["edge_id"].as_i64().unwrap().to_string()}));
    rebind_results_digest(&mut falsely_authoritative);
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
    case["product_disposition"]["actual"]["receipts"]
        .as_array_mut()
        .expect("actual product receipts")
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
    rebind_results_digest(&mut value);

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

    let mut caller_path = report();
    caller_path["cases"][0]["receipt_evidence"]["observed_receipts"][0]["source"]["project_file_components"] =
        json!(["src", "wrong.rs"]);
    caller_path["cases"][0]["receipt_evidence"]["observed_receipts"][0]["line_window"]["project_file_components"] =
        json!(["src", "wrong.rs"]);
    QualificationSummaryV1::from_json(caller_path)
        .expect_err("an exact oracle claim binds the caller declaration path");

    let mut indexed_drift = report();
    indexed_drift["cases"][0]["receipt_evidence"]["observed_receipts"][0]["line_window"]["observed_sha256"] =
        json!("c".repeat(64));
    QualificationSummaryV1::from_json(indexed_drift)
        .expect_err("indexed and observed whole-file hashes must agree");

    let mut oracle_selector = report();
    oracle_selector["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]
        ["oracle_step"]["caller"]["selector"] =
        json!({"kind":"canonical_id","canonical_id":"wrong-canonical"});
    QualificationSummaryV1::from_json(oracle_selector)
        .expect_err("an exact oracle claim binds the full caller selector");
}

#[test]
fn actionable_gap_keeps_the_exact_first_unproven_coordinate() {
    let mut value = report();
    let case = &mut value["cases"][10];
    assert_eq!(case["attempted_step_count"], 2);
    let missing = case["receipt_evidence"]["observed_receipts"]
        .as_array()
        .unwrap()
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
        "kind":"unknown",
        "gaps":["relation_missing"],
        "authoritative_receipts":[],
        "actual":{
            "kind":"unknown",
            "contract_digest":SHA,
            "gaps":[
                {"kind":"direct_call_missing","step_index":1},
                {"kind":"direct_call_missing","step_index":0}
            ],
            "connected_receipts":[]
        }
    });
    case["actionable_exact_gap"] = json!({
        "gap":{"kind":"direct_call_missing","step_index":0},
        "boundary":{"kind":"step","step_index":0}
    });
    case["receipt_evidence"]["observed_receipts"] = json!([]);
    for step in case["proof_trace"]["steps"]
        .as_array_mut()
        .expect("step traces")
    {
        step["candidate_edge_ids"] = json!([]);
        step["outcome"] = json!({
            "kind":"first_zero_survivor","gate":"raw_admission","histogram":[]
        });
    }
    rebuild_funnel(&mut value);
    rebind_results_digest(&mut value);
    QualificationSummaryV1::from_json(value.clone())
        .expect("the next prefix boundary is selected independent of gap order");

    value["cases"][10]["actionable_exact_gap"] = json!({
        "gap":{"kind":"direct_call_missing","step_index":1},
        "boundary":{"kind":"step","step_index":1}
    });
    rebind_results_digest(&mut value);
    QualificationSummaryV1::from_json(value)
        .expect_err("a later gap is not actionable at the current prefix boundary");
}

#[test]
fn actionable_gap_requires_the_trace_to_explain_the_exact_gap_cause() {
    let mut claimed = report();
    let claimed_receipt = claimed["cases"][0]["receipt_evidence"]["observed_receipts"][0].clone();
    make_non_proven_case(&mut claimed, "unknown", Some("source_binding"));
    claimed["cases"][0]["receipt_evidence"]["observed_receipts"] = json!([claimed_receipt]);
    claimed["cases"][0]["proof_trace"]["steps"][0]["candidate_edge_ids"] = json!([1]);
    claimed["cases"][0]["proof_trace"]["steps"][0]["outcome"] =
        json!({"kind":"admitted","edge_ids":[1]});
    rebuild_funnel(&mut claimed);
    rebind_results_digest(&mut claimed);
    QualificationSummaryV1::from_json(claimed)
        .expect_err("an admitted step cannot explain a source-window gap");

    let mut measured = report();
    let admitted_receipt = measured["cases"][0]["receipt_evidence"]["observed_receipts"][0].clone();
    make_non_proven_case(&mut measured, "unknown", Some("source_binding"));
    measured["cases"][0]["actionable_exact_gap"] = Value::Null;
    measured["cases"][0]["receipt_evidence"]["observed_receipts"] = json!([admitted_receipt]);
    measured["cases"][0]["proof_trace"]["steps"][0]["candidate_edge_ids"] = json!([1]);
    measured["cases"][0]["proof_trace"]["steps"][0]["outcome"] =
        json!({"kind":"admitted","edge_ids":[1]});
    rebuild_funnel(&mut measured);
    rebind_results_digest(&mut measured);
    let parsed = QualificationSummaryV1::from_json(measured)
        .expect("a trace-disposition mismatch remains measurable as a hard-gate failure");
    assert!(
        !parsed.cases[0]
            .evaluable_facts()
            .expect("derive product-disposition evidence")
            .product_disposition_matches_evidence,
        "a product gap whose cause is absent from the trace must fail the disposition gate"
    );
}

#[test]
fn selector_and_finalization_gap_causes_are_trace_bound() {
    let mut selector = report();
    make_non_proven_case(&mut selector, "unknown", Some("selector_missing"));
    selector["cases"][0]["proof_trace"]["selectors"][0]["outcome"] =
        json!({"kind":"failed","reason":"ambiguous"});
    rebind_results_digest(&mut selector);
    QualificationSummaryV1::from_json(selector)
        .expect_err("selector_missing cannot be explained by an ambiguous selector trace");

    let mut budget = report();
    set_receipt_budget_case(&mut budget["cases"][0]);
    budget["cases"][0]["proof_trace"]["finalization"] =
        json!({"kind":"failed","failure":"receipt_integration"});
    rebind_results_digest(&mut budget);
    QualificationSummaryV1::from_json(budget)
        .expect_err("an integration failure cannot explain output_budget_exceeded");
}

#[test]
fn output_budget_is_actionable_only_after_every_step_was_admitted() {
    let mut value = report();
    set_receipt_budget_case(&mut value["cases"][0]);
    let mut case: contracts::CaseReportV1 =
        serde_json::from_value(value["cases"][0].clone()).expect("budget case DTO");
    assert!(
        contracts::actionable_exact_gap_for_case(
            &case.product_disposition,
            &case.receipt_evidence,
            case.attempted_step_count,
            &case.proof_trace,
        )
        .expect("derive budget boundary")
        .is_some()
    );

    case.proof_trace.steps[0].outcome = contracts::StepQualificationOutcomeV1::FirstZeroSurvivor {
        gate: contracts::CandidateGateV1::RawAdmission,
        histogram: Vec::new(),
    };
    assert_eq!(
        contracts::actionable_exact_gap_for_case(
            &case.product_disposition,
            &case.receipt_evidence,
            case.attempted_step_count,
            &case.proof_trace,
        )
        .expect("derive non-actionable budget boundary"),
        None
    );
}

#[test]
fn missing_oracle_steps_are_separate_exact_rows() {
    let mut value = report();
    let case = &mut value["cases"][0];
    let contract_digest = case["product_disposition"]["actual"]["contract_digest"].clone();
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
        "kind":"unknown","gaps":["relation_missing"],"authoritative_receipts":[],
        "actual":{"kind":"unknown","contract_digest":contract_digest,"gaps":[{"kind":"direct_call_missing","step_index":0}]}
    });
    case["actionable_exact_gap"] = json!({
        "gap":{"kind":"direct_call_missing","step_index":0},
        "boundary":{"kind":"step","step_index":0}
    });
    case["proof_trace"]["steps"][0]["outcome"] = json!({
        "kind":"first_zero_survivor","gate":"raw_admission",
        "histogram":[{"reason":{"kind":"raw_admission","reason":"wrong_kind"},"edge_ids":[1]}]
    });
    rebuild_funnel(&mut value);
    rebind_results_digest(&mut value);

    let mut omitted = value.clone();
    omitted["cases"][0]["receipt_evidence"]["missing_oracle_steps"] = json!([]);
    QualificationSummaryV1::from_json(omitted)
        .expect_err("an uncovered oracle step requires separate missing evidence");
    let mut wrong_oracle = value.clone();
    wrong_oracle["cases"][0]["receipt_evidence"]["missing_oracle_steps"][0]["oracle_step"]["target"]
        ["symbol"] = json!("crate::wrong_target");
    rebind_results_digest(&mut wrong_oracle);
    QualificationSummaryV1::from_json(wrong_oracle)
        .expect("missing row is structurally closed")
        .validate_against_oracle(
            &CorpusV1::from_json(corpus()).expect("corpus"),
            &parsed_path_files(),
        )
        .expect_err("missing row must carry the exact frozen oracle data");

    let parsed = QualificationSummaryV1::from_json(value).expect("separate missing oracle row");
    parsed
        .validate_against_oracle(
            &CorpusV1::from_json(corpus()).expect("corpus"),
            &parsed_path_files(),
        )
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
        rebind_results_digest(&mut value);
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
    rebind_results_digest(&mut exact);
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
    case["product_disposition"]["actual"]["receipts"] = json!([]);
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
    rebind_results_digest(&mut truncated);
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
    let mut projected_receipts = Vec::with_capacity(6);
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
                projected_receipts.push(json!({
                    "receipt_id":receipt["receipt_id"],
                    "edge_id":edge_id.to_string()
                }));
            }
            observed_receipts.push(receipt);
        }
    }
    case["receipt_evidence"]["observed_receipts"] = Value::Array(observed_receipts);
    case["product_disposition"]["authoritative_receipts"] = Value::Array(authoritative_receipts);
    case["product_disposition"]["actual"]["receipts"] = Value::Array(projected_receipts);
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
