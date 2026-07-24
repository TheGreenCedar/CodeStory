use super::super::{ScenarioArtifact, ScenarioOrchestration};
use super::analysis::resident_generation_is_valid;
use super::evidence::validate_named_evidence;
use super::{ScenarioEvidence, opaque_measurement_sample_id};
use crate::qualification::request::{QualificationContracts, REQUIRED_SCENARIOS};
use std::collections::BTreeSet;

#[test]
fn measurement_sample_ids_are_opaque_stable_and_unique_between_runs() {
    let first =
        opaque_measurement_sample_id(&"a".repeat(64), "hosted_linux_x64_cpu", "warm_query_ipc", 1);
    let second_run =
        opaque_measurement_sample_id(&"b".repeat(64), "hosted_linux_x64_cpu", "warm_query_ipc", 1);
    let duplicate =
        opaque_measurement_sample_id(&"a".repeat(64), "hosted_linux_x64_cpu", "warm_query_ipc", 1);
    assert_ne!(first, second_run);
    assert_eq!(first, duplicate);
    assert_eq!(first.len(), 64);
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn true_idle_measurement_requires_a_resident_positive_generation() {
    assert!(!resident_generation_is_valid("listening", None));
    assert!(!resident_generation_is_valid("resident", None));
    assert!(!resident_generation_is_valid("resident", Some(0)));
    assert!(!resident_generation_is_valid("draining", Some(1)));
    assert!(resident_generation_is_valid("resident", Some(1)));
}

#[test]
fn a_generic_operation_alias_cannot_satisfy_any_named_scenario() {
    let mut generic = ScenarioEvidence::default();
    generic.transitions.insert("generic_query_completed".into());
    generic
        .transitions
        .insert("generic_observe_completed".into());
    for scenario in REQUIRED_SCENARIOS {
        let error = validate_named_evidence(scenario, &generic)
            .expect_err("generic evidence must not satisfy a named scenario");
        assert!(
            error
                .to_string()
                .contains("embedding_qualification_named_evidence_incomplete")
        );
    }
}

#[test]
fn named_scenarios_require_their_fault_controls() {
    let cases = [
        ("frozen_owner", "freeze_owner"),
        ("incompatible_owner", "force_incompatible"),
        ("mixed_queue", "hold_class:query"),
        ("server_crash", "crash_server"),
        ("worker_stall", "stall_native"),
    ];
    for (scenario, required_control) in cases {
        let mut evidence = complete_evidence(scenario);
        evidence.controls.remove(required_control);
        assert!(validate_named_evidence(scenario, &evidence).is_err());
    }
}

#[test]
fn scenario_artifact_schema_has_raw_fields_without_verdicts() {
    let value = serde_json::to_value(ScenarioArtifact {
        schema_version: 3,
        scenario: "cold_race".into(),
        contracts: QualificationContracts {
            protocol_sha256: "a".repeat(64),
            constant_set_sha256: "b".repeat(64),
            measurement_protocol_sha256: "c".repeat(64),
        },
        orchestration: ScenarioOrchestration {
            started_ns: 1,
            finished_ns: 2,
            process_invocations: Vec::new(),
        },
        control_events: Vec::new(),
        process_observations: Vec::new(),
        observations: Vec::new(),
        events: Vec::new(),
    })
    .expect("serialize scenario artifact");
    let object = value.as_object().expect("artifact object");
    assert_eq!(
        object.keys().cloned().collect::<BTreeSet<_>>(),
        [
            "schema_version",
            "scenario",
            "contracts",
            "orchestration",
            "control_events",
            "process_observations",
            "observations",
            "events",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    for forbidden in ["status", "pass", "passed", "assertions", "core_scenario"] {
        assert!(!object.contains_key(forbidden));
    }
}

fn complete_evidence(scenario: &str) -> ScenarioEvidence {
    let (controls, transitions): (&[&str], &[&str]) = match scenario {
        "frozen_owner" => (
            &["freeze_owner", "release_owner"],
            &["bounded_owner_unresponsive", "owner_identity_stable"],
        ),
        "incompatible_owner" => (
            &["force_incompatible", "clear_incompatible"],
            &[
                "active_owner_rejected",
                "idle_owner_draining",
                "compatible_replacement",
            ],
        ),
        "mixed_queue" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "queues_saturated",
                "query_selected_before_bulk_backlog",
                "typed_capacity_retry_observed",
                "per_class_fifo_observed",
                "global_fifo_across_projects",
                "query_preference_observed",
                "bulk_resumed",
            ],
        ),
        "server_crash" => (
            &["hold_class:query", "crash_server"],
            &[
                "inflight_request_observed",
                "server_replaced",
                "query_replayed",
            ],
        ),
        "worker_stall" => (
            &["stall_native", "release_native"],
            &[
                "stalled_request_observed",
                "watchdog_fail_stop_observed",
                "unrelated_process_survived",
                "post_stall_replacement",
            ],
        ),
        _ => unreachable!("test covers fault-controlled scenarios"),
    };
    ScenarioEvidence {
        controls: controls.iter().map(|value| (*value).into()).collect(),
        transitions: transitions.iter().map(|value| (*value).into()).collect(),
    }
}
