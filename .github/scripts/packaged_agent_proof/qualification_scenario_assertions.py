"""Assertion owners for qualification scenarios."""

from __future__ import annotations

from dataclasses import dataclass

from .foundation import (
    HEX_SHA256,
    require,
)
from .contracts import (
    require_nonnegative_int,
    require_positive_int,
    require_sha256,
)
from .qualification_scenario_evidence import (
    ScenarioAssertionEvidence,
    validate_replay_attempts,
    validate_retry_state,
)


def _client_death_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("dead_client_work_observed")
    continued = evidence.transition(
        "other_client_continued",
        {"project_identity_sha256"},
    )
    terminated = evidence.transition("client_terminated", {"termination"})
    reclaimed = evidence.scheduler("dead_client_work_reclaimed")
    post = evidence.transition(
        "post_reclaim_other_client_query",
        {"server_instance_id"},
    )
    return {
        "dead_client_queue_and_leases_reclaimed": (
            active["query_depth"] > 0
            and active["bulk_depth"] > 0
            and active["active_request_count"] > 0
            and active["lease_count"] > 0
            and reclaimed["query_depth"] == 0
            and reclaimed["bulk_depth"] == 0
            and reclaimed["active_request_count"] == 0
            and reclaimed["lease_count"] == 0
            and terminated["termination"] == "terminated"
        ),
        "other_client_continues": (
            HEX_SHA256.fullmatch(str(continued["project_identity_sha256"]))
            is not None
            and post["server_instance_id"] in evidence.snapshot_instances
        ),
        "no_server_replacement": len(evidence.snapshot_instances) == 1,
    }


def _frozen_owner_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    bounded = evidence.transition(
        "bounded_owner_unresponsive",
        {
            "started_ns",
            "finished_ns",
            "error_code",
            "timeout_ms",
            "clock_domain",
            "clock_boot_id",
            "retry",
        },
    )
    stable = evidence.transition(
        "owner_identity_stable",
        {
            "server_instance_id",
            "lifetime_authority_id",
            "listener_id",
            "pid",
            "process_start_id",
            "post_release_query_succeeded",
        },
    )
    started = require_nonnegative_int(
        bounded["started_ns"],
        "frozen owner started_ns",
    )
    finished = require_nonnegative_int(
        bounded["finished_ns"],
        "frozen owner finished_ns",
    )
    timeout_ms = require_positive_int(
        bounded["timeout_ms"],
        "frozen owner timeout_ms",
    )
    stable_pid = require_positive_int(stable["pid"], "frozen owner stable pid")
    retry = validate_retry_state(bounded["retry"], "frozen owner retry")
    stable_identity = (
        len(evidence.snapshot_instances) == 1
        and stable["server_instance_id"] == next(iter(evidence.snapshot_instances))
        and len(evidence.snapshot_authorities) == 1
        and (
            stable["lifetime_authority_id"],
            stable["listener_id"],
        )
        == next(iter(evidence.snapshot_authorities))
        and all(
            snapshot["process"]["pid"] == stable_pid
            and snapshot["process"]["process_start_id"]
            == stable["process_start_id"]
            for snapshot in evidence.snapshots
        )
        and stable["post_release_query_succeeded"] is True
    )
    return {
        "owner_unresponsive_is_bounded": (
            finished >= started
            and finished - started <= timeout_ms * 1_000_000
            and bounded["clock_domain"] == "awake_monotonic"
            and bool(bounded["clock_boot_id"])
            and bounded["error_code"] == "embedding_server_owner_unresponsive"
            and retry.code == bounded["error_code"]
            and retry.retry_class == "after_server_change"
            and bool(retry.retry_condition)
        ),
        "authority_retained": stable_identity,
        "no_unlink": stable_identity,
        "no_pid_kill": stable_identity,
        "no_takeover": stable_identity,
        "no_second_engine": len(evidence.snapshot_engines) == 1,
    }


def _incompatible_owner_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.transition(
        "active_owner_rejected",
        {"compatibility_evidence", "error_code", "retry"},
    )
    idle = evidence.transition(
        "idle_owner_draining",
        {"compatibility_evidence", "error_code", "retry"},
    )
    replacement = evidence.transition(
        "compatible_replacement",
        {"old_server_instance_id", "new_server_instance_id"},
    )
    replaced = (
        replacement["old_server_instance_id"]
        != replacement["new_server_instance_id"]
        and {
            replacement["old_server_instance_id"],
            replacement["new_server_instance_id"],
        }
        <= evidence.snapshot_instances
    )
    active_retry = validate_retry_state(
        active["retry"],
        "incompatible active retry",
    )
    idle_retry = validate_retry_state(
        idle["retry"],
        "incompatible idle retry",
    )
    expected_condition = "the incompatible server exits while fully idle"
    return {
        "idle_owner_drains": (
            idle["compatibility_evidence"] == "injected_contract_mismatch"
            and idle["error_code"] == "embedding_server_draining"
            and idle_retry.code == idle["error_code"]
            and idle_retry.retry_class == "after_owner_idle"
            and idle_retry.retry_after_ms == 0
            and idle_retry.retry_condition == expected_condition
            and replaced
        ),
        "active_owner_returns_typed_retry": (
            active["compatibility_evidence"] == "injected_contract_mismatch"
            and active["error_code"]
            == "embedding_server_incompatible_active_owner"
            and active_retry.code == active["error_code"]
            and active_retry.retry_class == "after_owner_idle"
            and active_retry.retry_after_ms == 0
            and active_retry.retry_condition == expected_condition
        ),
        "one_authority": len(evidence.snapshot_authorities) <= 2 and replaced,
        "one_engine_maximum": len(evidence.snapshot_instances) == 2 and replaced,
    }


def _server_crash_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("inflight_request_observed")
    replacement = evidence.transition(
        "server_replaced",
        {"old_server_instance_id", "new_server_instance_id"},
    )
    replay = evidence.transition(
        "query_replayed",
        {
            "logical_operation_count",
            "wire_attempt_count",
            "wire_attempts",
        },
    )
    attempts = validate_replay_attempts(
        replay,
        old_server_instance_id=replacement["old_server_instance_id"],
        new_server_instance_id=replacement["new_server_instance_id"],
    )
    return {
        "one_replacement_server": (
            active["active_request_class"] == "query"
            and replacement["old_server_instance_id"]
            != replacement["new_server_instance_id"]
            and [attempt.server_instance_id for attempt in attempts]
            == [
                replacement["old_server_instance_id"],
                replacement["new_server_instance_id"],
            ]
        ),
        "pure_embedding_rpc_replayed_at_most_once": (
            replay["logical_operation_count"] == 1
            and replay["wire_attempt_count"] <= 2
            and sum(attempt.outcome == "completed" for attempt in attempts) == 1
        ),
    }


@dataclass(frozen=True)
class ColdRaceElection:
    instances: frozenset[str]
    authorities: frozenset[tuple[str, str]]
    engines: frozenset[tuple[str, str, int, int]]


def _cold_race_election(
    evidence: ScenarioAssertionEvidence,
) -> ColdRaceElection:
    witnesses = {
        phase: [
            observation
            for observation in evidence.process_observations
            if observation.get("phase") == phase
        ]
        for phase in ("cold_race_first", "cold_race_second")
    }
    require(
        all(len(phase_witnesses) == 1 for phase_witnesses in witnesses.values()),
        "cold race must retain exactly one post-reset snapshot from each process",
    )
    snapshots = tuple(
        witnesses[phase][0]["snapshot"]
        for phase in ("cold_race_first", "cold_race_second")
    )
    require(
        all(
            isinstance(snapshot, dict) and snapshot.get("engine") is not None
            for snapshot in snapshots
        ),
        "cold race post-reset snapshots must retain engine identity",
    )
    return ColdRaceElection(
        instances=frozenset(
            snapshot["process"]["server_instance_id"] for snapshot in snapshots
        ),
        authorities=frozenset(
            (
                snapshot["authority"]["lifetime_authority_id"],
                snapshot["authority"]["listener_id"],
            )
            for snapshot in snapshots
        ),
        engines=frozenset(
            (
                snapshot["engine"]["engine_owner_id"],
                snapshot["engine"]["native_worker_id"],
                snapshot["engine"]["load_generation"],
                snapshot["engine"]["model_load_count"],
            )
            for snapshot in snapshots
        ),
    )


def _cold_race_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    election = _cold_race_election(evidence)
    independent = evidence.transition(
        "two_independent_processes",
        {
            "first_pid",
            "second_pid",
            "first_project_identity_sha256",
            "second_project_identity_sha256",
            "first_transport_peer_verified",
            "second_transport_peer_verified",
        },
    )
    converged = evidence.transition(
        "single_server_convergence",
        {"server_instance_id", "lifetime_authority_id"},
    )
    hosts = (
        evidence.same_account.get("plugin_hosts")
        if isinstance(evidence.same_account, dict)
        else None
    )
    return {
        "two_independent_plugin_hosts": (
            require_positive_int(independent["first_pid"], "cold race first pid")
            != require_positive_int(independent["second_pid"], "cold race second pid")
            and independent["first_transport_peer_verified"] is True
            and independent["second_transport_peer_verified"] is True
        ),
        "same_os_account": (
            evidence.same_account.get("relation") == "same_os_account"
            and isinstance(hosts, list)
            and len(hosts) == 2
        ),
        "different_repositories": (
            independent["first_project_identity_sha256"]
            != independent["second_project_identity_sha256"]
            and all(
                HEX_SHA256.fullmatch(str(independent[field])) is not None
                for field in (
                    "first_project_identity_sha256",
                    "second_project_identity_sha256",
                )
            )
        ),
        "one_lifetime_authority": (
            len(election.authorities) == 1
            and converged["lifetime_authority_id"]
            == next(iter(election.authorities))[0]
        ),
        "one_listener": (
            len({identity[1] for identity in election.authorities}) == 1
        ),
        "one_server": (
            len(election.instances) == 1
            and converged["server_instance_id"] == next(iter(election.instances))
        ),
        "one_engine_owner": (
            len({identity[0] for identity in election.engines}) == 1
        ),
        "one_native_worker": (
            len({identity[1] for identity in election.engines}) == 1
        ),
        "one_load_generation": (
            len({identity[2] for identity in election.engines}) == 1
        ),
        "one_model_load": (
            len(election.engines) == 1 and next(iter(election.engines))[3] == 1
        ),
    }


def _mixed_queue_capacity_is_typed(capacity: dict) -> bool:
    for queue_class in ("query", "bulk"):
        record = capacity[f"{queue_class}_65th"]
        pressure = (
            record.get("error", {}).get("capacity")
            if isinstance(record, dict)
            else None
        )
        if not (
            isinstance(pressure, dict)
            and pressure.get("queue_class") == queue_class
            and pressure.get("capacity") == 64
            and pressure.get("depth") == 64
            and bool(pressure.get("retry_condition"))
        ):
            return False
    return True


def _mixed_queue_is_fifo(class_orders: dict) -> bool:
    return all(
        class_orders[f"{queue_class}_expected_queue_insertion_request_ids"]
        == class_orders[f"{queue_class}_native_completed_request_ids"]
        and isinstance(
            class_orders[
                f"{queue_class}_expected_queue_insertion_request_ids"
            ],
            list,
        )
        and bool(
            class_orders[
                f"{queue_class}_expected_queue_insertion_request_ids"
            ]
        )
        and isinstance(
            class_orders[f"{queue_class}_native_completion_sequences"],
            list,
        )
        and bool(class_orders[f"{queue_class}_native_completion_sequences"])
        and all(
            isinstance(sequence, int)
            and not isinstance(sequence, bool)
            and sequence > 0
            for sequence in class_orders[
                f"{queue_class}_native_completion_sequences"
            ]
        )
        and class_orders[f"{queue_class}_native_completion_sequences"]
        == sorted(class_orders[f"{queue_class}_native_completion_sequences"])
        and len(
            set(class_orders[f"{queue_class}_native_completion_sequences"])
        )
        == len(class_orders[f"{queue_class}_native_completion_sequences"])
        for queue_class in ("query", "bulk")
    )


def _mixed_queue_preserves_project_order(project_orders: dict) -> bool:
    return all(
        project_orders[
            f"{queue_class}_expected_queue_insertion_project_identities"
        ]
        == project_orders[f"{queue_class}_native_completed_project_identities"]
        and len(
            set(
                project_orders[
                    f"{queue_class}_expected_queue_insertion_project_identities"
                ]
            )
        )
        == 2
        for queue_class in ("query", "bulk")
    )


def _mixed_queue_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    saturated = evidence.scheduler("queues_saturated")
    selected = evidence.scheduler("query_selected_before_bulk_backlog")
    capacity = evidence.transition(
        "typed_capacity_retry_observed",
        {"query_65th", "bulk_65th"},
    )
    class_orders = evidence.transition(
        "per_class_fifo_observed",
        {
            "query_expected_queue_insertion_request_ids",
            "query_native_completed_request_ids",
            "query_native_completion_sequences",
            "bulk_expected_queue_insertion_request_ids",
            "bulk_native_completed_request_ids",
            "bulk_native_completion_sequences",
        },
    )
    project_orders = evidence.transition(
        "global_fifo_across_projects",
        {
            "query_expected_queue_insertion_project_identities",
            "query_native_completed_project_identities",
            "bulk_expected_queue_insertion_project_identities",
            "bulk_native_completed_project_identities",
        },
    )
    preference = evidence.transition(
        "query_preference_observed",
        {
            "first_query_request_id",
            "first_query_native_completion_sequence",
            "first_bulk_request_id",
            "first_bulk_native_completion_sequence",
        },
    )
    resumed = evidence.transition(
        "bulk_resumed",
        {
            "last_query_request_id",
            "last_query_native_completion_sequence",
            "last_bulk_request_id",
            "last_bulk_native_completion_sequence",
        },
    )
    return {
        "query_and_bulk_capacities_are_64": (
            saturated["query_capacity"] == saturated["query_depth"] == 64
            and saturated["bulk_capacity"] == saturated["bulk_depth"] == 64
        ),
        "fifo_within_each_class": _mixed_queue_is_fifo(class_orders),
        "query_preferred_between_bulk_batches": (
            selected["active_request_class"] == "query"
            and selected["bulk_depth"] > 0
            and preference["first_query_native_completion_sequence"]
            < preference["first_bulk_native_completion_sequence"]
        ),
        "bulk_resumes_when_query_queue_permits": (
            resumed["last_bulk_native_completion_sequence"]
            > resumed["last_query_native_completion_sequence"]
        ),
        "no_project_or_scope_round_robin": (
            _mixed_queue_preserves_project_order(project_orders)
        ),
        "typed_retry_names_useful_condition": (
            _mixed_queue_capacity_is_typed(capacity)
        ),
        "no_project_or_request_text_leakage": all(
            all(HEX_SHA256.fullmatch(str(value)) is not None for value in values)
            for key, values in project_orders.items()
            if key.endswith("_project_identities")
        ),
    }


@dataclass(frozen=True)
class WorkerStallMeasurements:
    old_pid: int
    marker_sha256: str
    watchdog_observed_ns: int
    watchdog_last_progress_ns: int
    hard_no_progress_ms: int
    watchdog_cadence_ms: int


def _worker_stall_measurements(fail_stop: dict) -> WorkerStallMeasurements:
    require_nonnegative_int(
        fail_stop["watchdog_progress_sequence"],
        "worker stall watchdog progress sequence",
    )
    return WorkerStallMeasurements(
        old_pid=require_positive_int(
            fail_stop["old_pid"],
            "worker stall old pid",
        ),
        marker_sha256=require_sha256(
            fail_stop["watchdog_marker_sha256"],
            "worker stall watchdog marker",
        ),
        watchdog_observed_ns=require_nonnegative_int(
            fail_stop["watchdog_observed_ns"],
            "worker stall watchdog observed_ns",
        ),
        watchdog_last_progress_ns=require_nonnegative_int(
            fail_stop["watchdog_last_progress_ns"],
            "worker stall watchdog last progress",
        ),
        hard_no_progress_ms=require_positive_int(
            fail_stop["hard_native_no_progress_ms"],
            "worker stall hard no-progress bound",
        ),
        watchdog_cadence_ms=require_positive_int(
            fail_stop["watchdog_cadence_ms"],
            "worker stall watchdog cadence",
        ),
    )


def _worker_stall_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("stalled_request_observed")
    fail_stop = evidence.transition(
        "watchdog_fail_stop_observed",
        {
            "old_pid",
            "old_server_instance_id",
            "wire_attempt_count",
            "wire_attempts",
            "watchdog_marker_sha256",
            "watchdog_reason",
            "watchdog_observed_ns",
            "watchdog_last_progress_ns",
            "watchdog_progress_sequence",
            "hard_native_no_progress_ms",
            "watchdog_cadence_ms",
        },
    )
    replacement = evidence.transition(
        "post_stall_replacement",
        {"new_server_instance_id"},
    )
    survivor = evidence.transition(
        "unrelated_process_survived",
        {"pid", "process_start_id", "new_server_instance_id"},
    )
    measurements = _worker_stall_measurements(fail_stop)
    attempts = validate_replay_attempts(
        fail_stop,
        old_server_instance_id=fail_stop["old_server_instance_id"],
        new_server_instance_id=replacement["new_server_instance_id"],
    )
    replacement_observed = (
        replacement["new_server_instance_id"] in evidence.snapshot_instances
        and all(
            snapshot["process"]["pid"] != measurements.old_pid
            for snapshot in evidence.snapshots[-1:]
        )
    )
    return {
        "independent_watchdog_fail_stops_server": (
            active["active_request_class"] == "bulk"
            and bool(measurements.marker_sha256)
            and fail_stop["watchdog_reason"] == "embedding_engine_stalled"
            and measurements.watchdog_observed_ns
            >= measurements.watchdog_last_progress_ns
            and measurements.watchdog_observed_ns
            - measurements.watchdog_last_progress_ns
            >= measurements.hard_no_progress_ms * 1_000_000
            and measurements.watchdog_cadence_ms
            < measurements.hard_no_progress_ms
            and attempts[0].outcome == "server_loss"
            and replacement_observed
        ),
        "unrelated_process_survives": (
            replacement_observed
            and require_positive_int(
                survivor["pid"],
                "worker stall survivor pid",
            )
            != measurements.old_pid
            and bool(survivor["process_start_id"])
            and survivor["new_server_instance_id"]
            == replacement["new_server_instance_id"]
            and any(
                invocation.get("pid") == survivor["pid"]
                and invocation.get("process_start_id")
                == survivor["process_start_id"]
                and invocation.get("operation") == "query"
                and invocation.get("termination") == "exited"
                for invocation in evidence.invocations
            )
        ),
        "pure_embedding_rpc_replayed_at_most_once": (
            fail_stop["wire_attempt_count"] <= 2
            and sum(attempt.outcome == "completed" for attempt in attempts) == 1
        ),
    }


@dataclass(frozen=True)
class TrueIdleTransitions:
    active: dict
    preserved: dict
    reclaimed: dict
    waited: dict
    idle_surfaces: dict
    absent: dict
    respawned: dict

    @classmethod
    def from_evidence(
        cls,
        evidence: ScenarioAssertionEvidence,
    ) -> TrueIdleTransitions:
        return cls(
            active=evidence.scheduler("anti_idle_work_observed"),
            preserved=evidence.transition(
                "owner_preserved_across_idle_boundary",
                {
                    "held_started_ns",
                    "held_observed_ns",
                    "contract_idle_timeout_ms",
                    "server_instance_id",
                },
            ),
            reclaimed=evidence.scheduler("anti_idle_work_reclaimed"),
            waited=evidence.transition(
                "true_idle_wait",
                {
                    "server_idle_epoch_ns",
                    "server_idle_elapsed_before_client_wait_ns",
                    "client_wait_required_ns",
                    "client_wait_elapsed_ns",
                    "contract_idle_timeout_ms",
                    "clock_boot_id",
                },
            ),
            idle_surfaces=evidence.transition(
                "idle_surfaces_exercised",
                {
                    "diagnostic_count",
                    "idle_connection_close_count",
                    "last_diagnostic_client_elapsed_ns",
                    "last_idle_connection_close_client_elapsed_ns",
                },
            ),
            absent=evidence.transition(
                "owner_absent_after_true_idle",
                {"old_server_instance_id"},
            ),
            respawned=evidence.transition(
                "server_respawned",
                {
                    "new_server_instance_id",
                    "load_generation",
                    "model_load_count",
                    "materialized_model_sha256",
                    "materialized_reused",
                },
            ),
        )


@dataclass(frozen=True)
class TrueIdleWitnesses:
    absent_observed_ns: int
    absent_transition_ns: int
    respawn_observed_ns: int
    respawn_transition_ns: int
    respawn_snapshot: dict
    post_absence_invocations: tuple[dict, ...]


def _true_idle_witnesses(
    evidence: ScenarioAssertionEvidence,
) -> TrueIdleWitnesses:
    absent_observations = [
        observation
        for observation in evidence.process_observations
        if observation.get("phase") == "true_idle_after_wait"
    ]
    require(
        len(absent_observations) == 1
        and absent_observations[0].get("snapshot") is None,
        "true idle must retain exactly one absent-owner witness",
    )
    respawn_observations = [
        observation
        for observation in evidence.process_observations
        if observation.get("phase") == "true_idle_respawned"
    ]
    require(
        len(respawn_observations) == 1
        and isinstance(respawn_observations[0].get("snapshot"), dict)
        and respawn_observations[0]["snapshot"].get("engine") is not None,
        "true idle must retain exactly one replacement-engine witness",
    )
    absent_transition = evidence.observations_by_kind[
        "owner_absent_after_true_idle"
    ][0]
    respawn_transition = evidence.observations_by_kind["server_respawned"][0]
    absent_transition_ns = require_nonnegative_int(
        absent_transition.get("observed_ns"),
        "true idle absence transition time",
    )
    respawn_transition_ns = require_nonnegative_int(
        respawn_transition.get("observed_ns"),
        "true idle respawn transition time",
    )
    return TrueIdleWitnesses(
        absent_observed_ns=require_nonnegative_int(
            absent_observations[0].get("observed_ns"),
            "true idle absent-owner witness time",
        ),
        absent_transition_ns=absent_transition_ns,
        respawn_observed_ns=require_nonnegative_int(
            respawn_observations[0].get("observed_ns"),
            "true idle replacement witness time",
        ),
        respawn_transition_ns=respawn_transition_ns,
        respawn_snapshot=respawn_observations[0]["snapshot"],
        post_absence_invocations=tuple(
            invocation
            for invocation in evidence.invocations
            if isinstance(invocation.get("started_ns"), int)
            and not isinstance(invocation.get("started_ns"), bool)
            and absent_transition_ns
            <= invocation["started_ns"]
            <= respawn_transition_ns
        ),
    )


@dataclass(frozen=True)
class TrueIdleMeasurements:
    timeout_ms: int
    diagnostic_count: int
    idle_connection_close_count: int
    last_diagnostic_client_elapsed_ns: int
    last_idle_connection_close_client_elapsed_ns: int
    server_idle_elapsed_before_client_wait_ns: int
    client_wait_required_ns: int
    client_wait_elapsed_ns: int
    respawn_load_generation: int
    respawn_model_load_count: int
    respawn_materialized_sha256: str


def _true_idle_measurements(
    transitions: TrueIdleTransitions,
) -> TrueIdleMeasurements:
    require_nonnegative_int(
        transitions.waited["server_idle_epoch_ns"],
        "true idle server epoch",
    )
    return TrueIdleMeasurements(
        timeout_ms=require_positive_int(
            transitions.waited["contract_idle_timeout_ms"],
            "true idle contract timeout",
        ),
        diagnostic_count=require_positive_int(
            transitions.idle_surfaces["diagnostic_count"],
            "true idle diagnostic count",
        ),
        idle_connection_close_count=require_positive_int(
            transitions.idle_surfaces["idle_connection_close_count"],
            "true idle connection close count",
        ),
        last_diagnostic_client_elapsed_ns=require_nonnegative_int(
            transitions.idle_surfaces["last_diagnostic_client_elapsed_ns"],
            "true idle last diagnostic ns",
        ),
        last_idle_connection_close_client_elapsed_ns=require_nonnegative_int(
            transitions.idle_surfaces[
                "last_idle_connection_close_client_elapsed_ns"
            ],
            "true idle last connection close ns",
        ),
        server_idle_elapsed_before_client_wait_ns=require_nonnegative_int(
            transitions.waited["server_idle_elapsed_before_client_wait_ns"],
            "true idle server elapsed before local wait",
        ),
        client_wait_required_ns=require_nonnegative_int(
            transitions.waited["client_wait_required_ns"],
            "true idle client wait required",
        ),
        client_wait_elapsed_ns=require_nonnegative_int(
            transitions.waited["client_wait_elapsed_ns"],
            "true idle client wait elapsed",
        ),
        respawn_load_generation=require_positive_int(
            transitions.respawned["load_generation"],
            "true idle respawn load generation",
        ),
        respawn_model_load_count=require_positive_int(
            transitions.respawned["model_load_count"],
            "true idle respawn model load count",
        ),
        respawn_materialized_sha256=require_sha256(
            transitions.respawned["materialized_model_sha256"],
            "true idle respawn materialized model",
        ),
    )


def _true_idle_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    transitions = TrueIdleTransitions.from_evidence(evidence)
    witnesses = _true_idle_witnesses(evidence)
    measurements = _true_idle_measurements(transitions)
    invocation = (
        witnesses.post_absence_invocations[0]
        if len(witnesses.post_absence_invocations) == 1
        else None
    )
    respawn_engine = witnesses.respawn_snapshot["engine"]
    return {
        "queued_active_and_leased_work_prevent_exit": (
            transitions.active["query_depth"] > 0
            and transitions.active["bulk_depth"] > 0
            and transitions.active["active_request_count"] > 0
            and transitions.active["lease_count"] > 0
            and transitions.preserved["server_instance_id"]
            == transitions.absent["old_server_instance_id"]
            and transitions.preserved["held_observed_ns"]
            - transitions.preserved["held_started_ns"]
            >= transitions.preserved["contract_idle_timeout_ms"] * 1_000_000
        ),
        "idle_connections_and_diagnostics_do_not_extend_idle": (
            measurements.diagnostic_count >= 2
            and measurements.idle_connection_close_count >= 2
            and measurements.last_diagnostic_client_elapsed_ns
            >= measurements.timeout_ms * 500_000
            and measurements.last_idle_connection_close_client_elapsed_ns
            >= measurements.timeout_ms * 500_000
            and bool(transitions.waited["clock_boot_id"])
        ),
        "exit_after_60000_awake_ms": (
            measurements.timeout_ms == 60_000
            and measurements.server_idle_elapsed_before_client_wait_ns
            + measurements.client_wait_required_ns
            >= measurements.timeout_ms * 1_000_000
            and measurements.client_wait_elapsed_ns
            >= measurements.client_wait_required_ns
            and transitions.reclaimed["query_depth"] == 0
            and transitions.reclaimed["bulk_depth"] == 0
            and transitions.reclaimed["active_request_count"] == 0
            and transitions.reclaimed["lease_count"] == 0
        ),
        "next_product_operation_respawns_without_consent": (
            transitions.absent["old_server_instance_id"]
            != transitions.respawned["new_server_instance_id"]
            and witnesses.absent_observed_ns <= witnesses.absent_transition_ns
            and invocation is not None
            and invocation.get("operation") == "query"
            and invocation.get("exit_code") == 0
            and invocation.get("termination") == "exited"
            and isinstance(invocation.get("finished_ns"), int)
            and not isinstance(invocation.get("finished_ns"), bool)
            and invocation["started_ns"]
            <= invocation["finished_ns"]
            <= witnesses.respawn_observed_ns
            <= witnesses.respawn_transition_ns
            and witnesses.respawn_snapshot["process"]["server_instance_id"]
            == transitions.respawned["new_server_instance_id"]
        ),
        "verified_materialization_reused": (
            isinstance(evidence.materialization, dict)
            and measurements.respawn_materialized_sha256
            == require_sha256(
                evidence.materialization.get("sha256"),
                "retained materialized model",
            )
            and transitions.respawned["materialized_reused"] is True
            and measurements.respawn_load_generation
            == respawn_engine["load_generation"]
            and measurements.respawn_model_load_count
            == respawn_engine["model_load_count"]
            == 1
        ),
    }


def derive_scenario_assertions(
    scenario_id: str,
    *,
    observations_by_kind: dict[str, list[dict]],
    process_observations: list[dict],
    invocations: list[dict],
    control_actions: list[str],
    same_account: dict,
    materialization: dict,
) -> dict[str, bool]:
    evidence = ScenarioAssertionEvidence.from_raw(
        scenario_id,
        observations_by_kind=observations_by_kind,
        process_observations=process_observations,
        invocations=invocations,
        same_account=same_account,
        materialization=materialization,
    )
    handler = {
        "client_death": _client_death_assertions,
        "cold_race": _cold_race_assertions,
        "frozen_owner": _frozen_owner_assertions,
        "incompatible_owner": _incompatible_owner_assertions,
        "mixed_queue": _mixed_queue_assertions,
        "server_crash": _server_crash_assertions,
        "true_idle_respawn": _true_idle_assertions,
        "worker_stall": _worker_stall_assertions,
    }.get(scenario_id)
    require(handler is not None, f"unknown qualification scenario {scenario_id}")
    assertions = handler(evidence)
    failed = sorted(name for name, value in assertions.items() if value is not True)
    require(
        not failed,
        f"qualification scenario {scenario_id} raw evidence failed assertions: {', '.join(failed)}",
    )
    return assertions
