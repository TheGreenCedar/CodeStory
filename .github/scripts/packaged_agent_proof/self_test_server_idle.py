"""True-idle server lifecycle self-tests."""

from __future__ import annotations

import json

from .foundation import ProofFailure, require
from .qualification import derive_scenario_assertions
from .self_test_full_stack_types import ServerIdentityFixture, TrueIdleFixture


def _true_idle_snapshots(
    first_snapshot: dict,
) -> tuple[dict, dict, list[dict]]:
    true_idle_before = json.loads(json.dumps(first_snapshot))
    true_idle_respawned = json.loads(json.dumps(first_snapshot))
    true_idle_respawned["process"]["server_instance_id"] = "respawned-server"
    true_idle_respawned["process"]["pid"] = 303
    true_idle_respawned["process"]["process_start_id"] = "process-start-303"
    true_idle_respawned["authority"]["lifetime_authority_id"] = "respawned-authority"
    true_idle_respawned["authority"]["listener_id"] = "respawned-listener"
    true_idle_respawned["engine"]["engine_owner_id"] = "respawned-engine-owner"
    true_idle_respawned["engine"]["native_worker_id"] = "respawned-native-worker"
    true_idle_process_observations = [
        {
            "phase": "true_idle_before",
            "observed_ns": 100,
            "snapshot": true_idle_before,
        },
        {
            "phase": "true_idle_after_wait",
            "observed_ns": 200,
            "snapshot": None,
        },
        {
            "phase": "true_idle_respawned",
            "observed_ns": 400,
            "snapshot": true_idle_respawned,
        },
    ]
    return true_idle_before, true_idle_respawned, true_idle_process_observations


def _true_idle_transitions(
    before: dict,
    respawned: dict,
) -> tuple[dict, str]:
    true_idle_before = before
    true_idle_respawned = respawned
    active_scheduler = {
        "query_capacity": 64,
        "query_depth": 1,
        "bulk_capacity": 64,
        "bulk_depth": 1,
        "active_request_count": 1,
        "lease_count": 1,
        "active_request_class": "query",
    }
    reclaimed_scheduler = {
        "query_capacity": 64,
        "query_depth": 0,
        "bulk_capacity": 64,
        "bulk_depth": 0,
        "active_request_count": 0,
        "lease_count": 0,
        "active_request_class": None,
    }
    materialized_sha256 = "c" * 64
    true_idle_transitions = {
        "anti_idle_work_observed": [{"values": active_scheduler}],
        "owner_preserved_across_idle_boundary": [
            {
                "values": {
                    "held_started_ns": 0,
                    "held_observed_ns": 60_000_000_000,
                    "contract_idle_timeout_ms": 60_000,
                    "server_instance_id": true_idle_before["process"][
                        "server_instance_id"
                    ],
                }
            }
        ],
        "anti_idle_work_reclaimed": [{"values": reclaimed_scheduler}],
        "true_idle_wait": [
            {
                "values": {
                    "server_idle_epoch_ns": 1,
                    "server_idle_elapsed_before_client_wait_ns": 59_000_000_000,
                    "client_wait_required_ns": 1_000_000_000,
                    "client_wait_elapsed_ns": 1_000_000_000,
                    "contract_idle_timeout_ms": 60_000,
                    "clock_boot_id": "boot-1",
                }
            }
        ],
        "idle_surfaces_exercised": [
            {
                "values": {
                    "diagnostic_count": 2,
                    "idle_connection_close_count": 2,
                    "last_diagnostic_client_elapsed_ns": 30_000_000_000,
                    "last_idle_connection_close_client_elapsed_ns": 30_000_000_000,
                }
            }
        ],
        "owner_absent_after_true_idle": [
            {
                "observed_ns": 225,
                "values": {
                    "old_server_instance_id": true_idle_before["process"][
                        "server_instance_id"
                    ]
                },
            }
        ],
        "server_respawned": [
            {
                "observed_ns": 450,
                "values": {
                    "new_server_instance_id": true_idle_respawned["process"][
                        "server_instance_id"
                    ],
                    "load_generation": 1,
                    "model_load_count": 1,
                    "materialized_model_sha256": materialized_sha256,
                    "materialized_reused": True,
                },
            }
        ],
    }
    return true_idle_transitions, materialized_sha256


def _verified_true_idle_fixture(
    first_snapshot: dict,
) -> TrueIdleFixture:
    before, respawned, true_idle_process_observations = _true_idle_snapshots(
        first_snapshot
    )
    true_idle_transitions, materialized_sha256 = _true_idle_transitions(
        before, respawned
    )
    true_idle_invocations = [
        {
            "operation": "query",
            "started_ns": 250,
            "finished_ns": 350,
            "exit_code": 0,
            "termination": "exited",
        },
        {
            "operation": "query",
            "started_ns": 10,
            "finished_ns": 20,
            "exit_code": 0,
            "termination": "exited",
        },
    ]
    true_idle_assertions = derive_scenario_assertions(
        "true_idle_respawn",
        observations_by_kind=true_idle_transitions,
        process_observations=true_idle_process_observations,
        invocations=true_idle_invocations,
        same_account={},
        materialization={
            "sha256": materialized_sha256,
            "reused_on_rejoin": False,
        },
    )
    require(
        all(true_idle_assertions.values()),
        "cold first-use state contaminated replacement materialization proof",
    )
    return TrueIdleFixture(
        transitions=true_idle_transitions,
        process_observations=true_idle_process_observations,
        invocations=true_idle_invocations,
        materialized_sha256=materialized_sha256,
    )


def _materialization_binding_hostiles(fixture: TrueIdleFixture) -> None:
    true_idle_transitions = fixture.transitions
    true_idle_process_observations = fixture.process_observations
    true_idle_invocations = fixture.invocations
    materialized_sha256 = fixture.materialized_sha256
    for field, hostile_value in (
        ("materialized_reused", False),
        ("materialized_model_sha256", "d" * 64),
    ):
        hostile_transitions = json.loads(json.dumps(true_idle_transitions))
        hostile_transitions["server_respawned"][0]["values"][field] = hostile_value
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=hostile_transitions,
                process_observations=true_idle_process_observations,
                invocations=true_idle_invocations,
                same_account={},
                materialization={
                    "sha256": materialized_sha256,
                    "reused_on_rejoin": False,
                },
            )
        except ProofFailure as error:
            require(
                str(error)
                == "qualification scenario true_idle_respawn raw evidence failed assertions: verified_materialization_reused",
                f"hostile true-idle {field} changed its exact failure",
            )
        else:
            raise ProofFailure(f"hostile true-idle {field} escaped replacement binding")


def _temporal_ordering_hostiles(fixture: TrueIdleFixture) -> None:
    true_idle_transitions = fixture.transitions
    true_idle_process_observations = fixture.process_observations
    true_idle_invocations = fixture.invocations
    materialized_sha256 = fixture.materialized_sha256
    historical_only_invocations = json.loads(json.dumps(true_idle_invocations[1:]))
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=true_idle_process_observations,
            invocations=historical_only_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "historical true-idle query changed its exact temporal failure",
        )
    else:
        raise ProofFailure("historical query satisfied true-idle respawn proof")
    failed_then_successful_invocations = [
        {
            "operation": "query",
            "started_ns": 230,
            "finished_ns": 240,
            "exit_code": 1,
            "termination": "exited",
        },
        *true_idle_invocations,
    ]
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=true_idle_process_observations,
            invocations=failed_then_successful_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "failed first true-idle query changed its exact failure",
        )
    else:
        raise ProofFailure("failed first query was hidden by a later respawn success")
    historical_respawn_transition = json.loads(json.dumps(true_idle_transitions))
    historical_respawn_transition["server_respawned"][0]["observed_ns"] = 150
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=historical_respawn_transition,
            process_observations=true_idle_process_observations,
            invocations=true_idle_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "historical true-idle respawn transition changed its temporal failure",
        )
    else:
        raise ProofFailure("historical respawn transition was accepted")


def _absence_cardinality_hostile(fixture: TrueIdleFixture) -> None:
    true_idle_transitions = fixture.transitions
    true_idle_process_observations = fixture.process_observations
    true_idle_invocations = fixture.invocations
    materialized_sha256 = fixture.materialized_sha256
    duplicate_absence = json.loads(json.dumps(true_idle_process_observations))
    duplicate_absence.insert(
        -1,
        {
            "phase": "true_idle_after_wait",
            "observed_ns": 225,
            "snapshot": None,
        },
    )
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=duplicate_absence,
            invocations=true_idle_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error) == "true idle must retain exactly one absent-owner witness",
            "duplicate true-idle absence changed its cardinality failure",
        )
    else:
        raise ProofFailure("duplicate true-idle absence witness was accepted")


def run_true_idle_self_tests(server: ServerIdentityFixture) -> None:
    fixture = _verified_true_idle_fixture(server.first_snapshot)
    _materialization_binding_hostiles(fixture)
    _temporal_ordering_hostiles(fixture)
    _absence_cardinality_hostile(fixture)
