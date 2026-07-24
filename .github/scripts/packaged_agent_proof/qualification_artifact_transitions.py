"""Qualification transition, event, and scenario-requirement validation."""

from __future__ import annotations

from collections import Counter

from .contracts import (
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
)
from .foundation import require
from .qualification_artifact_types import (
    QualificationControlEvidence,
    QualificationOrchestration,
    QualificationTransitionEvidence,
)


def _qualification_transitions(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
) -> QualificationTransitionEvidence:
    require(
        isinstance(value, list),
        f"qualification artifact {name} observations are malformed",
    )
    by_kind: dict[str, list[dict]] = {}
    for index, observation in enumerate(value):
        field = f"qualification artifact {name} observation {index}"
        require(isinstance(observation, dict), f"{field} is malformed")
        require_exact_keys(
            observation,
            {"sequence", "kind", "observed_ns", "values"},
            field,
        )
        require(
            observation["sequence"] == index,
            f"qualification artifact {name} observation sequence is not contiguous",
        )
        kind = require_nonempty_string(observation["kind"], f"{field}.kind")
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"{field}.observed_ns",
        )
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        require(
            isinstance(observation["values"], dict),
            f"{field}.values is malformed",
        )
        by_kind.setdefault(kind, []).append(observation)
    return QualificationTransitionEvidence(tuple(value), by_kind)


_REQUIRED_TRANSITIONS = {
    "client_death": {
        "dead_client_work_observed",
        "other_client_continued",
        "client_terminated",
        "dead_client_work_reclaimed",
        "post_reclaim_other_client_query",
    },
    "cold_race": {"two_independent_processes", "single_server_convergence"},
    "frozen_owner": {"bounded_owner_unresponsive", "owner_identity_stable"},
    "incompatible_owner": {
        "active_owner_rejected",
        "idle_owner_draining",
        "compatible_replacement",
    },
    "mixed_queue": {
        "queues_saturated",
        "query_selected_before_bulk_backlog",
        "typed_capacity_retry_observed",
        "per_class_fifo_observed",
        "global_fifo_across_projects",
        "query_preference_observed",
        "bulk_resumed",
    },
    "server_crash": {
        "inflight_request_observed",
        "server_replaced",
        "query_replayed",
    },
    "true_idle_respawn": {
        "anti_idle_work_observed",
        "owner_preserved_across_idle_boundary",
        "anti_idle_work_reclaimed",
        "true_idle_wait",
        "idle_surfaces_exercised",
        "owner_absent_after_true_idle",
        "server_respawned",
    },
    "worker_stall": {
        "stalled_request_observed",
        "watchdog_fail_stop_observed",
        "unrelated_process_survived",
        "post_stall_replacement",
    },
}

_REQUIRED_CONTROLS = {
    "client_death": Counter({"hold_class": 2, "release_class": 2}),
    "cold_race": Counter(),
    "frozen_owner": Counter({"freeze_owner": 1, "release_owner": 1}),
    "incompatible_owner": Counter({"force_incompatible": 1, "clear_incompatible": 1}),
    "mixed_queue": Counter({"hold_class": 2, "release_class": 2}),
    "server_crash": Counter({"hold_class": 1, "crash_server": 1}),
    "true_idle_respawn": Counter({"hold_class": 2, "release_class": 2}),
    "worker_stall": Counter({"stall_native": 1, "release_native": 1}),
}


def _verify_cold_race_evidence(
    *,
    name: str,
    process_observations: tuple[dict, ...],
    transitions: QualificationTransitionEvidence,
) -> None:
    require(
        any(
            observation["phase"] == "cold_race_no_owner"
            and observation["snapshot"] is None
            for observation in process_observations
        ),
        f"qualification artifact {name} did not prove owner absence before the race",
    )
    independent = transitions.by_kind["two_independent_processes"][0]["values"]
    require(
        independent.get("first_pid") != independent.get("second_pid")
        and independent.get("first_project_identity_sha256")
        != independent.get("second_project_identity_sha256")
        and independent.get("first_transport_peer_verified") is True
        and independent.get("second_transport_peer_verified") is True,
        f"qualification artifact {name} cold-race processes were not independent",
    )


def _verify_scenario_artifact_requirements(
    *,
    name: str,
    scenario_id: str,
    controls: QualificationControlEvidence,
    process_observations: tuple[dict, ...],
    transitions: QualificationTransitionEvidence,
) -> None:
    require(
        all(
            len(transitions.by_kind.get(kind, [])) == 1
            for kind in _REQUIRED_TRANSITIONS[scenario_id]
        ),
        f"qualification artifact {name} omitted or duplicated required raw transitions",
    )
    actual_controls = Counter(controls.actions)
    require(
        all(
            actual_controls[action] >= count
            for action, count in _REQUIRED_CONTROLS[scenario_id].items()
        ),
        f"qualification artifact {name} omitted required authenticated controls",
    )
    if scenario_id == "cold_race":
        _verify_cold_race_evidence(
            name=name,
            process_observations=process_observations,
            transitions=transitions,
        )


def _qualification_events(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list) and value,
        f"qualification artifact {name} has no correlated events",
    )
    for index, event in enumerate(value):
        field = f"qualification artifact {name} event {index}"
        require(isinstance(event, dict), f"{field} is malformed")
        require_exact_keys(
            event,
            {
                "sequence",
                "source",
                "action",
                "observed_ns",
                "correlation_id",
                "values",
            },
            field,
        )
        require(
            event["sequence"] == index,
            f"qualification artifact {name} event sequence is not contiguous",
        )
        require_nonempty_string(event["source"], f"{field}.source")
        require_nonempty_string(event["action"], f"{field}.action")
        observed_ns = require_nonnegative_int(
            event["observed_ns"], f"{field}.observed_ns"
        )
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        require(
            event["correlation_id"] is None
            or (
                isinstance(event["correlation_id"], str)
                and bool(event["correlation_id"])
            ),
            f"{field}.correlation_id is malformed",
        )
        require(isinstance(event["values"], dict), f"{field}.values is malformed")
    return tuple(value)
