"""True-idle qualification assertions."""

from __future__ import annotations

from dataclasses import dataclass

from .contracts import require_nonnegative_int, require_positive_int, require_sha256
from .foundation import require
from .qualification_scenario_evidence import ScenarioAssertionEvidence


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
    absent_transition = evidence.observations_by_kind["owner_absent_after_true_idle"][0]
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
            transitions.idle_surfaces["last_idle_connection_close_client_elapsed_ns"],
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
