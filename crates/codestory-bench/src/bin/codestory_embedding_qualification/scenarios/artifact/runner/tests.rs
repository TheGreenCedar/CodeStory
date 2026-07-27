use super::super::{
    CONTROL_TIMEOUT, QUEUE_SETUP_TIMEOUT, SNAPSHOT_TIMEOUT, ScenarioArtifact, ScenarioOrchestration,
};
use super::analysis::resident_generation_is_valid;
use super::evidence::validate_named_evidence;
use super::measurements::{
    declared_phase_boundaries, declared_workload_id, measurement_span_interval,
};
use super::process::{
    LOAD_ESTABLISHMENT_WAITS, busy_retry_worker_timeout, dead_client_setup_timeout,
    load_establishment_budget, load_establishment_timeout, measurement_worker_timeout,
    published_control_events,
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
    for operation in ["query", "observe", "measure_hello", "measure_query_frame"] {
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
        "resident_identity",
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
    // The true_idle respawn scenario spawns the `resident_identity` worker,
    // whose whole body is one `EnsureResident` exchange that the client bounds
    // by the contract bulk deadline. Regression: that operation name matched no
    // arm and fell through to the query deadline, so its coordinator budget was
    // 57s against a worker worst case of ~512s — the coordinator-kills-a-
    // progressing-worker defect this function exists to eliminate, reintroduced
    // by a name the match never saw.
    assert_eq!(
        measurement_worker_timeout("resident_identity"),
        measurement_worker_timeout("measure_resident_identity"),
        "the scenario residency probe must carry the same bulk-deadline budget as the residency measurement"
    );
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
fn load_establishment_budgets_cover_every_establishing_client_chain() {
    // The mixed_queue `mixed_queue_seed_active` wait — and every sibling that
    // gates on freshly spawned clients reaching the owner — bounds a phase,
    // not a single observation: each client pays its own process start and
    // executable/transport captures, then its contract connect and
    // spawn-convergence allowances, before its request is admitted or its lease
    // is granted, and the coordinator's own in-flight poll is one more observe
    // worker that can spend the whole snapshot allowance first. Regression:
    // calibration run 30238772170 (hosted_linux_x64_cpu) died with
    // embedding_qualification_snapshot_timeout:mixed_queue_seed_active at the
    // flat 20s budget while the seed client was legitimately still starting.
    let budgets = EmbeddingClientBudgets::current();
    for (phase, establishing_clients) in LOAD_ESTABLISHMENT_WAITS {
        let budget = load_establishment_timeout(phase).expect("classified wait");
        // Independently composed chain for this site: one start-and-capture
        // allowance per client the predicate gates on (each still a full
        // executable and transport hash, because capture reuse is unmerged),
        // the contract connect and spawn-convergence allowances paid to reach
        // the owner, and one more snapshot allowance for the coordinator's poll
        // already in flight.
        let mut chain = budgets
            .connect
            .saturating_add(budgets.spawn)
            .saturating_add(SNAPSHOT_TIMEOUT);
        for _ in 0..*establishing_clients {
            chain = chain.saturating_add(SNAPSHOT_TIMEOUT);
        }
        assert!(
            budget >= chain,
            "{phase}: budget must cover all {establishing_clients} establishing start-and-capture chains plus connect, spawn and the in-flight poll"
        );
        // The exact defect: the flat budget bounded a whole establishing phase
        // with the allowance the coordinator sizes for one observe worker.
        assert!(
            budget > SNAPSHOT_TIMEOUT,
            "{phase}: budget must strictly dominate the flat snapshot wait it replaced"
        );
    }
    // Each additional establishing client buys at least another whole
    // start-and-capture allowance, so a site's budget tracks its client count
    // instead of being one magnitude that happens to clear the single-client
    // chain. Regression: `true_idle_work_held` gates on two clients ramping
    // concurrently inside its own wait window and was given the single-client
    // budget, leaving the same flake class live in true_idle_respawn.
    for establishing_clients in 1..=4_u32 {
        assert!(
            load_establishment_budget(establishing_clients.saturating_add(1))
                >= load_establishment_budget(establishing_clients).saturating_add(SNAPSHOT_TIMEOUT),
            "each additional establishing client must buy another start-and-capture allowance"
        );
    }
    assert!(
        load_establishment_timeout("true_idle_work_held").expect("classified wait")
            > load_establishment_timeout("mixed_queue_seed_active").expect("classified wait"),
        "the two-client establishment wait must strictly dominate a single-client one"
    );
    // Fail closed: a wait whose establishing clients nobody counted has no
    // derived worst case, so it must not be handed a default budget.
    assert!(
        load_establishment_timeout("true_idle_work_hel").is_err(),
        "an unclassified wait must not resolve to a budget"
    );
    // Family ordering: a wait that needs a seeded queue rather than one
    // admission or lease carries the strictly larger queue-setup budget.
    assert!(
        dead_client_setup_timeout() >= load_establishment_budget(1),
        "the seeded-queue budget must dominate the single-admission budget in the same wait family"
    );
}

/// Every snapshot wait in the driver that is not worker-driven load
/// establishment, with the budget expression its call site must carry. Seeded
/// queue waits bound a client filling a queue and take the queue-setup budget;
/// observation waits hold no client ramp in the predicate path, so the flat
/// snapshot budget dominates their honest chain and each site states why.
const NON_LOAD_ESTABLISHMENT_WAITS: &[(&str, &str)] = &[
    ("client_death_lease_active", "dead_client_setup_timeout()"),
    ("mixed_queue_first_project_enqueued", "QUEUE_SETUP_TIMEOUT"),
    ("mixed_queue_saturated", "QUEUE_SETUP_TIMEOUT"),
    ("client_death_lease_reclaimed", "SNAPSHOT_TIMEOUT"),
    ("frozen_owner_released", "SNAPSHOT_TIMEOUT"),
    ("incompatible_owner_idle", "SNAPSHOT_TIMEOUT"),
    ("mixed_queue_query_selected", "SNAPSHOT_TIMEOUT"),
    ("true_idle_work_reclaimed", "SNAPSHOT_TIMEOUT"),
];

#[test]
fn load_establishment_waits_are_classified_at_every_site() {
    // The family split has to be auditable at the sites, not only in prose:
    // every snapshot wait in the driver is either worker-driven load
    // establishment — a budget derived from the number of clients its predicate
    // gates on — or it is not, and then it is queue seeding or an observation
    // whose predicate path holds no client ramp. Regression: the flat snapshot
    // budget that killed calibration run 30238772170 at mixed_queue_seed_active
    // could be restored at a single call site without touching any derivation,
    // and no other assertion in this suite would notice.
    let mut sited = BTreeSet::new();
    for (file, source) in runner_sources() {
        for (phase, _) in wait_sites(&source, "wait_for_established_load") {
            assert!(
                LOAD_ESTABLISHMENT_WAITS
                    .iter()
                    .any(|(wait, _)| *wait == phase),
                "{file}: {phase} spends the load-establishment budget with no derived client count"
            );
            assert!(sited.insert(phase.clone()), "{file}: {phase} waits twice");
        }
        for call in [
            "wait_for_snapshot",
            "wait_for_control_snapshot",
            "wait_for_true_idle_epoch",
        ] {
            for (phase, budget) in wait_sites(&source, call) {
                let expected = NON_LOAD_ESTABLISHMENT_WAITS
                    .iter()
                    .find(|(wait, _)| *wait == phase)
                    .map(|(_, budget)| *budget)
                    .unwrap_or_else(|| {
                        panic!("{file}: {phase} is an unclassified snapshot wait carrying {budget}")
                    });
                assert_eq!(
                    budget, expected,
                    "{file}: {phase} must carry {expected}, not {budget}"
                );
                assert!(sited.insert(phase.clone()), "{file}: {phase} waits twice");
            }
        }
    }
    for (phase, _) in LOAD_ESTABLISHMENT_WAITS {
        assert!(
            sited.contains(*phase),
            "{phase} carries a derived load-establishment budget that no site spends"
        );
    }
    for (phase, _) in NON_LOAD_ESTABLISHMENT_WAITS {
        assert!(
            sited.contains(*phase),
            "{phase} is classified as a non-establishment wait that no site performs"
        );
    }
}

/// Every source file of the scenario driver, read from disk so a wait added in
/// a file nobody remembered to list still has to be classified.
fn runner_sources() -> Vec<(String, String)> {
    let directory = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/bin/codestory_embedding_qualification/scenarios/artifact/runner"
    ));
    let mut sources = std::fs::read_dir(directory)
        .expect("read the scenario runner directory")
        .map(|entry| entry.expect("scenario runner entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("scenario source name")
                .to_string_lossy()
                .into_owned();
            let source = std::fs::read_to_string(&path).expect("read a scenario source");
            (name, source)
        })
        .collect::<Vec<_>>();
    sources.sort();
    assert!(
        sources.len() >= 10,
        "scenario driver sources went missing from the classification scan"
    );
    sources
}

/// The `(phase, budget)` pair of every `self.<call>(` site in `source` whose
/// first argument is a phase literal. The budget is the next argument with its
/// whitespace removed; sites that take no budget argument yield the text that
/// follows the phase, which their caller ignores.
fn wait_sites(source: &str, call: &str) -> Vec<(String, String)> {
    let opening = format!("self.{call}(");
    let mut sites = Vec::new();
    let mut rest = source;
    while let Some(index) = rest.find(&opening) {
        rest = &rest[index.saturating_add(opening.len())..];
        let Some(quoted) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some(end) = quoted.find('"') else {
            continue;
        };
        let mut depth = 0_u32;
        let mut budget = String::new();
        for character in quoted[end.saturating_add(1)..]
            .chars()
            .skip_while(|character| *character != ',')
            .skip(1)
        {
            match character {
                '(' | '[' | '{' => depth = depth.saturating_add(1),
                ')' | ']' | '}' if depth == 0 => break,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => break,
                _ => {}
            }
            if !character.is_whitespace() {
                budget.push(character);
            }
        }
        sites.push((quoted[..end].to_owned(), budget));
    }
    sites
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

/// The exact record that tore on the Linux calibration cell in run
/// 30269041727: 242 bytes starting at offset 89916 of a 90159-byte log, so its
/// first 196 bytes end at 90112 — a 4 KiB page boundary — part-way through the
/// `boot_id` string value.
const TORN_HOLD_CLASS_RECORD: &str = "{\"schema_version\":1,\"sequence\":28,\"action\":\"hold_class\",\"status\":\"completed\",\"server_event_sequence\":17,\"clock\":{\"domain\":\"awake_monotonic\",\"api\":\"CLOCK_MONOTONIC\",\"boot_id\":\"cb5daa6d-70bb-4839-962e-9c5c3850a68d\",\"observed_ns\":3538471174799}}";

/// Bytes of that record the reader actually observed: the near-side page of
/// the torn append, cut inside a string literal.
const TORN_HOLD_CLASS_PREFIX_BYTES: usize = 196;

fn published_event_line(sequence: u64) -> String {
    format!(
        "{{\"schema_version\":1,\"sequence\":{sequence},\"action\":\"snapshot\",\"status\":\"completed\",\"server_event_sequence\":{sequence},\"clock\":{{\"domain\":\"awake_monotonic\",\"api\":\"CLOCK_MONOTONIC\",\"boot_id\":\"cb5daa6d-70bb-4839-962e-9c5c3850a68d\",\"observed_ns\":{sequence}}}}}"
    )
}

fn published_event_sequences(path: &std::path::Path) -> Vec<u64> {
    published_control_events(path)
        .expect("an append-only event log must read without error")
        .into_iter()
        .map(|event| event.sequence)
        .collect()
}

#[test]
fn a_half_written_event_line_reads_as_not_yet_written_instead_of_failing() {
    // Regression: calibration run 30269041727 (hosted_linux_x64_cpu) failed
    // `mixed_queue` with `parse embedding qualification control event: EOF
    // while parsing a string at line 1 column 196`. The preserved log is
    // wholly valid at rest; the control poll simply read it while the server
    // was still publishing the very `hold_class` response the poll was waiting
    // for, and a buffered append becomes visible page by page. The poll has to
    // treat that prefix as "not written yet" and come back, which is what its
    // `CONTROL_TIMEOUT` loop exists to do.
    let torn = &TORN_HOLD_CLASS_RECORD[..TORN_HOLD_CLASS_PREFIX_BYTES];
    let observed = serde_json::from_str::<super::super::ControlEvent>(torn)
        .expect_err("the near-side page of the torn append is not a parsable record");
    assert_eq!(
        observed.to_string(),
        "EOF while parsing a string at line 1 column 196",
        "the fixture must reproduce the calibration failure byte for byte, or this test guards nothing"
    );

    let directory = tempfile::tempdir().expect("temporary qualification directory");
    let path = directory.path().join("nonce.events.jsonl");

    // A log whose only content is a record still being published reads as
    // empty, not as a corrupt log.
    std::fs::write(&path, torn).expect("write partially published log");
    assert!(
        published_event_sequences(&path).is_empty(),
        "a log holding only an unfinished record has published nothing yet"
    );

    // Records completed ahead of the unfinished tail are still delivered.
    let published = format!(
        "{}\n{}\n",
        published_event_line(26),
        published_event_line(27)
    );
    std::fs::write(&path, format!("{published}{torn}")).expect("write torn log");
    assert_eq!(
        published_event_sequences(&path),
        vec![26, 27],
        "an unfinished trailing record must not hide the records already published"
    );

    // Once the writer finishes the record it appears, so tolerating the tail
    // delays the observation by one poll rather than losing it.
    std::fs::write(&path, format!("{published}{TORN_HOLD_CLASS_RECORD}\n"))
        .expect("write completed log");
    assert_eq!(
        published_event_sequences(&path),
        vec![26, 27, 28],
        "the completed record must be observed on the next poll"
    );
}

#[test]
fn a_complete_but_malformed_event_line_still_fails_the_read() {
    // The tolerance above is scoped to the one condition an append-only log
    // makes benign: bytes after the last newline, which the writer has not
    // finished publishing. A line the writer did terminate is a finished
    // record, so malformed content there is real corruption and must stay a
    // hard failure instead of silently shrinking the event history.
    let directory = tempfile::tempdir().expect("temporary qualification directory");
    let path = directory.path().join("nonce.events.jsonl");

    let interior = format!(
        "{}\n{{\"schema_version\":1,\"sequence\":27,\"action\":\n{}\n",
        published_event_line(26),
        published_event_line(28)
    );
    std::fs::write(&path, &interior).expect("write log with a malformed interior line");
    assert!(
        published_control_events(&path).is_err(),
        "a terminated but malformed interior record must fail the read"
    );

    let trailing = format!(
        "{}\n{{\"schema_version\":1,\"sequence\":27,\"action\":\n",
        published_event_line(26)
    );
    std::fs::write(&path, &trailing).expect("write log with a malformed final line");
    assert!(
        published_control_events(&path).is_err(),
        "a malformed record the writer terminated is corruption even when it is last"
    );
}
