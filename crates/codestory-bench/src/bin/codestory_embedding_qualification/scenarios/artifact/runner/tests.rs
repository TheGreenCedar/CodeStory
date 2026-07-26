use super::super::{
    CONTROL_TIMEOUT, QUEUE_SETUP_TIMEOUT, SNAPSHOT_TIMEOUT, ScenarioArtifact, ScenarioOrchestration,
};
use super::analysis::resident_generation_is_valid;
use super::evidence::validate_named_evidence;
use super::measurements::{
    declared_phase_boundaries, declared_workload_id, measurement_span_interval,
};
use super::process::{
    busy_retry_worker_timeout, dead_client_setup_timeout, measurement_worker_timeout,
};
use super::{ScenarioEvidence, WorkerOutput, opaque_measurement_sample_id};
use crate::qualification::request::{QualificationContracts, REQUIRED_METRICS, REQUIRED_SCENARIOS};
use codestory_retrieval::{
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, EmbeddingClientBudgets,
    EmbeddingQualificationWorkerMeasurementSpan, EmbeddingServerClockSnapshot,
    PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS, PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
};
use std::collections::BTreeSet;
use std::time::Duration;

#[test]
fn measurement_worker_budgets_dominate_the_deadlines_workers_honor() {
    // `wait_for_child` is a hung-worker watchdog: it must never kill a
    // measurement worker that is still inside its own contract deadlines
    // (connect or spawn convergence, then one request bounded by its class
    // deadline), plus the coordinator's snapshot and control allowances.
    // Regression: calibration run 30197324641 (hosted_linux_x64_cpu) was
    // killed by a flat 20s snapshot-wait budget while the warm-bulk workload
    // legitimately needs ~24s and the 256-document throughput workload ~96s.
    let budgets = EmbeddingClientBudgets::current();
    let orchestration_terms = budgets
        .connect
        .saturating_add(budgets.spawn)
        .saturating_add(SNAPSHOT_TIMEOUT)
        .saturating_add(CONTROL_TIMEOUT);
    for operation in [
        "query",
        "observe",
        "resident_identity",
        "measure_hello",
        "measure_query_frame",
    ] {
        assert!(
            measurement_worker_timeout(operation)
                >= orchestration_terms.saturating_add(budgets.query_request),
            "query-class measurement budget must dominate the worker's own query deadline chain"
        );
    }
    for operation in [
        "bulk",
        "measure_bulk_frame",
        "measure_spawn_hello",
        "measure_product_query",
        "measure_resident_identity",
    ] {
        let bulk_timeout = measurement_worker_timeout(operation);
        assert!(
            bulk_timeout >= orchestration_terms.saturating_add(budgets.bulk_request),
            "bulk-deadline measurement budget must dominate the worker's own bulk deadline chain"
        );
        // The exact defect: the coordinator killed bulk measurement workers
        // below the bulk request deadline the workers themselves honor.
        assert!(
            bulk_timeout >= Duration::from_millis(PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS),
            "bulk measurement budget must not undercut the contract bulk request deadline"
        );
    }
    // The true-idle measurement worker waits out the server's own idle
    // deadline before the absence observation; its watchdog must dominate
    // that self-enforced wait plus its quiescence and absence-grace waits.
    assert!(
        measurement_worker_timeout("measure_true_idle")
            >= Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
                .saturating_add(Duration::from_secs(30))
                .saturating_add(SNAPSHOT_TIMEOUT),
        "true-idle measurement budget must dominate the server idle deadline plus the worker's own waits"
    );
    // The busy-retry worker seeds the held queues (queue-setup phase), then
    // after release drains queries bounded by its own 120s per-request
    // deadline and replays under its 90s protocol deadline.
    assert!(
        busy_retry_worker_timeout()
            >= QUEUE_SETUP_TIMEOUT
                .saturating_add(Duration::from_millis(120_000))
                .saturating_add(Duration::from_millis(90_000)),
        "busy-retry measurement budget must dominate the worker's own self-enforced deadlines"
    );
}

#[test]
fn dead_client_setup_budget_dominates_the_seeding_terms_the_flat_wait_undercut() {
    // The `client_death_lease_active` wait bounds queue seeding by a freshly
    // spawned client, not one snapshot: before its lease and held work are
    // visible together the client pays its contract connect and
    // spawn-convergence allowances and each poll of the wait spends part of
    // the snapshot allowance. Regression: calibration run 30200986788
    // (protected macOS Metal cell) timed out at the flat 20s snapshot wait
    // while the dead client was legitimately still ramping its held load.
    let budgets = EmbeddingClientBudgets::current();
    assert!(
        dead_client_setup_timeout()
            >= budgets
                .connect
                .saturating_add(budgets.spawn)
                .saturating_add(SNAPSHOT_TIMEOUT),
        "dead-client setup budget must dominate the client's connect and spawn allowances plus the snapshot allowance"
    );
    // Load establishment is the same phase family as mixed_queue's gated
    // seeding, which bounds a strictly larger 64+64 enqueue with the
    // queue-setup budget; the dead-client wait must not fall back below it.
    assert!(
        dead_client_setup_timeout() >= QUEUE_SETUP_TIMEOUT,
        "dead-client load establishment is a queue-seeding phase and must carry the queue-setup budget"
    );
}

#[test]
fn recorded_phases_and_workloads_are_exactly_the_protocol_declarations() {
    // Regression for calibration run 30205779627: PR #1420 stamped every
    // metric with the invented generic pair
    // packaged_worker_operation_started/packaged_worker_operation_validated
    // and three non-protocol workload ids, which the checker rejects. The
    // WP5-floored frozen constants derive their meaning from the protocol's
    // declared phase boundaries, so the driver's tables must mirror
    // `phase_boundaries` and `workloads` in
    // crates/codestory-llama-sys/per-user-embedding-server-measurement-protocol.json
    // exactly — never a renamed protocol and never the generic worker window.
    let protocol: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../codestory-llama-sys/per-user-embedding-server-measurement-protocol.json"
        ))
        .expect("read the measurement protocol"),
    )
    .expect("parse the measurement protocol");
    let driver_metrics = REQUIRED_METRICS
        .iter()
        .copied()
        .filter(|metric| {
            !matches!(
                *metric,
                "retrieval_quality" | "total_codestory_process_memory"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(driver_metrics.len(), 11);
    for metric in driver_metrics {
        let declared = declared_phase_boundaries(metric).expect("declared phases");
        let expected = protocol["phase_boundaries"][metric]
            .as_array()
            .unwrap_or_else(|| panic!("protocol phase boundaries for {metric}"));
        assert_eq!(
            declared.to_vec(),
            expected
                .iter()
                .map(|phase| phase.as_str().expect("phase name"))
                .collect::<Vec<_>>(),
            "driver phase table diverged from the protocol for {metric}"
        );
        assert!(
            !declared.contains(&"packaged_worker_operation_started")
                && !declared.contains(&"packaged_worker_operation_validated"),
            "{metric} must not carry the generic whole-worker window phases"
        );
        assert_eq!(
            declared_workload_id(metric).expect("declared workload"),
            protocol["workloads"][metric]["workload_id"]
                .as_str()
                .expect("workload id"),
            "driver workload table diverged from the protocol for {metric}"
        );
    }
    for unknown in ["retrieval_quality", "total_codestory_process_memory"] {
        assert!(
            declared_phase_boundaries(unknown).is_err(),
            "externally owned metric {unknown} must not be recordable by the driver"
        );
    }
}

fn span_test_output() -> (WorkerOutput, EmbeddingQualificationWorkerMeasurementSpan) {
    let output = WorkerOutput {
        schema_version: EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
        pid: 42,
        process_start_id: "boot:42".into(),
        executable_sha256: "b".repeat(64),
        executable_version: "0.16.1".into(),
        project_identity_sha256: "c".repeat(64),
        clock: EmbeddingServerClockSnapshot {
            domain: "awake_monotonic".into(),
            api: "test".into(),
            boot_id: "boot".into(),
            resolution_ns: 1,
        },
        started_ns: 100,
        finished_ns: 1_000,
        inclusive_clock_api: "inclusive-test".into(),
        inclusive_started_ns: 90,
        inclusive_finished_ns: 1_010,
        boot_id_started: "boot".into(),
        boot_id_finished: "boot".into(),
        result: None,
        protocol_exchange: None,
        queue_operations: None,
        engine_identity: None,
        measurement: None,
        error: None,
    };
    let span = EmbeddingQualificationWorkerMeasurementSpan {
        awake_started_ns: 200,
        awake_finished_ns: 800,
        inclusive_started_ns: 195,
        inclusive_finished_ns: 805,
        boot_id_started: "boot".into(),
        boot_id_finished: "boot".into(),
    };
    (output, span)
}

#[test]
fn measurement_spans_must_sit_inside_the_worker_window_on_the_worker_clock() {
    let (output, span) = span_test_output();
    let interval = measurement_span_interval(&output, &span).expect("valid span interval");
    // The recorded sample carries the declared-instant sub-window, not the
    // whole-worker window that regressed run 30205779627.
    assert_eq!(interval.awake_started_ns, 200);
    assert_eq!(interval.awake_finished_ns, 800);
    assert_eq!(interval.inclusive_started_ns, 195);
    assert_eq!(interval.inclusive_finished_ns, 805);

    let mut reversed = span.clone();
    reversed.awake_started_ns = 900;
    let mut before_window = span.clone();
    before_window.awake_started_ns = 50;
    let mut after_window = span.clone();
    after_window.awake_finished_ns = 2_000;
    let mut inclusive_outside = span.clone();
    inclusive_outside.inclusive_started_ns = 10;
    let mut other_boot = span.clone();
    other_boot.boot_id_finished = "other-boot".into();
    for (hostile, case) in [
        (reversed, "reversed awake readings"),
        (before_window, "span starting before the worker window"),
        (after_window, "span finishing after the worker window"),
        (
            inclusive_outside,
            "inclusive readings outside the worker witness",
        ),
        (other_boot, "span crossing a boot identity"),
    ] {
        assert!(
            measurement_span_interval(&output, &hostile).is_err(),
            "accepted a measurement span with {case}"
        );
    }
}

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
