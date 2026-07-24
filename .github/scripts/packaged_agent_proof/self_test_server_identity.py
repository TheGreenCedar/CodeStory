"""Engine, server identity, and cold-election self-tests."""

from __future__ import annotations

import json

from .archive import verify_runtime_against_manifest
from .foundation import ProofFailure, require
from .process import engine_identity, server_snapshot, shared_server_identity
from .qualification import derive_scenario_assertions
from .self_test_full_stack_types import FullStackFixture, ServerIdentityFixture

def _engine_runtime_test(fixture: FullStackFixture) -> dict:
    manifest = fixture.manifest
    valid = {
        "embedding_model_sha256": "a" * 64,
        "embedding_ggml_build_identity": manifest["engine"]["build_identity"],
        "embedding_backend": "Metal",
        "embedding_adapter": "Apple GPU",
        "embedding_policy": "accelerated",
        "embedding_engine_instance_id": "engine-1",
        "embedding_engine_residency": "resident",
        "embedding_engine_load_generation": 1,
        "embedding_model_load_count": 1,
        "embedding_smoke_ms": 1.0,
        "embedding_initialization_ms": 2.0,
        "embedding_accelerator_execution_verified": True,
        "embedding_execution_devices": ["Apple GPU"],
        "embedding_execution_backends": ["Metal"],
        "embedding_execution_observation_source": "ggml_eval_callback",
        "embedding_encode_count": 1,
        "embedding_execution_node_count": 1,
        "embedding_resident_accelerator_tensor_count": 1,
        "embedding_resident_accelerator_tensor_bytes": 1,
        "embedding_model_layer_count": 13,
        "embedding_offloaded_layer_count": 13,
    }
    engine_identity(valid, "accelerated", "Metal")
    runtime = {
        "identity": valid,
        "second_host_identity": valid.copy(),
        "rejoin_identity": valid.copy(),
    }
    evidence = verify_runtime_against_manifest(manifest, runtime, "accelerated")
    require(evidence["execution"] == "proven_by_live_runtime", "runtime contract proof failed")
    return valid

def _shared_snapshot_test(
    fixture: FullStackFixture,
) -> tuple[dict, dict, dict]:
    manifest = fixture.manifest
    protocol_sha256 = fixture.protocol_sha256
    constant_set_sha256 = fixture.constant_set_sha256
    measurement_protocol_sha256 = fixture.measurement_protocol_sha256
    snapshot_payload = {
        "embedding_server": {
            "schema_version": 1,
            "event_sequence": 17,
            "lifecycle": "resident",
            "clock": {
                "domain": "awake_monotonic",
                "api": "mach_absolute_time",
                "boot_id": "boot-1",
                "resolution_ns": 1,
            },
            "protocol": {
                "bootstrap_version": 1,
                "schema_version": 1,
                "protocol_sha256": protocol_sha256,
                "constant_set_sha256": constant_set_sha256,
                "measurement_protocol_sha256": measurement_protocol_sha256,
            },
            "authority": {
                "endpoint_namespace_id": "endpoint-1",
                "lifetime_authority_id": "authority-1",
                "listener_id": "listener-1",
                "peer_verified": True,
            },
            "process": {
                "server_instance_id": "server-1",
                "pid": 101,
                "process_start_id": "boot-1:101",
                "executable_sha256": manifest["binary"]["sha256"],
                "executable_version": "0.0.0",
            },
            "scheduler": {
                "query_capacity": 64,
                "query_depth": 0,
                "bulk_capacity": 64,
                "bulk_depth": 0,
                "connection_count": 2,
                "active_request_count": 0,
                "lease_count": 0,
                "active_request": None,
            },
            "engine": {
                "engine_owner_id": "engine-owner-1",
                "native_worker_id": "native-worker-1",
                "load_generation": 1,
                "model_load_count": 1,
                "successful_encode_count": 3,
            },
            "failure": None,
        }
    }
    first_snapshot = server_snapshot(snapshot_payload, manifest, require_resident=True)
    second_snapshot = server_snapshot(
        json.loads(json.dumps(snapshot_payload)),
        manifest,
        require_resident=True,
    )
    shared = shared_server_identity(first_snapshot, second_snapshot)
    require(shared["model_load_count"] == 1, "shared server identity self-test failed")
    return snapshot_payload, first_snapshot, shared

def _cold_election_tests(first_snapshot: dict) -> tuple[list[dict], dict]:
    retired_snapshot = json.loads(json.dumps(first_snapshot))
    retired_snapshot["process"]["server_instance_id"] = "retired-server"
    retired_snapshot["authority"]["lifetime_authority_id"] = "retired-authority"
    retired_snapshot["authority"]["listener_id"] = "retired-listener"
    retired_snapshot["engine"]["engine_owner_id"] = "retired-engine-owner"
    retired_snapshot["engine"]["native_worker_id"] = "retired-native-worker"
    elected_snapshot = json.loads(json.dumps(first_snapshot))
    cold_process_observations = [
        {
            "phase": "cold_race_no_owner_before",
            "snapshot": retired_snapshot,
        },
        {"phase": "cold_race_no_owner", "snapshot": None},
        {"phase": "cold_race_first", "snapshot": elected_snapshot},
        {
            "phase": "cold_race_second",
            "snapshot": json.loads(json.dumps(elected_snapshot)),
        },
    ]
    cold_transitions = {
        "two_independent_processes": [
            {
                "values": {
                    "first_pid": 101,
                    "second_pid": 102,
                    "first_project_identity_sha256": "a" * 64,
                    "second_project_identity_sha256": "b" * 64,
                    "first_transport_peer_verified": True,
                    "second_transport_peer_verified": True,
                }
            }
        ],
        "single_server_convergence": [
            {
                "values": {
                    "server_instance_id": elected_snapshot["process"][
                        "server_instance_id"
                    ],
                    "lifetime_authority_id": elected_snapshot["authority"][
                        "lifetime_authority_id"
                    ],
                }
            }
        ],
    }
    cold_assertions = derive_scenario_assertions(
        "cold_race",
        observations_by_kind=cold_transitions,
        process_observations=cold_process_observations,
        invocations=[],
        same_account={
            "relation": "same_os_account",
            "plugin_hosts": [{"pid": 101}, {"pid": 102}],
        },
        materialization={},
    )
    require(
        all(cold_assertions.values()),
        "retired pre-race owner contaminated post-reset election assertions",
    )
    duplicate_phase_observations = json.loads(
        json.dumps(cold_process_observations)
    )
    duplicate_phase_observations.insert(
        -1, {"phase": "cold_race_first", "snapshot": None}
    )
    try:
        derive_scenario_assertions(
            "cold_race",
            observations_by_kind=cold_transitions,
            process_observations=duplicate_phase_observations,
            invocations=[],
            same_account={
                "relation": "same_os_account",
                "plugin_hosts": [{"pid": 101}, {"pid": 102}],
            },
            materialization={},
        )
    except ProofFailure as error:
        require(
            str(error)
            == "cold race must retain exactly one post-reset snapshot from each process",
            "duplicate cold-race phase changed its cardinality failure",
        )
    else:
        raise ProofFailure("duplicate cold-race phase observation was accepted")
    return cold_process_observations, cold_transitions

def _cold_split_test(
    process_observations: list[dict],
    transitions: dict,
) -> None:
    cold_process_observations = process_observations
    cold_transitions = transitions
    split_process_observations = json.loads(
        json.dumps(cold_process_observations)
    )
    split_snapshot = split_process_observations[-1]["snapshot"]
    split_snapshot["process"]["server_instance_id"] = "split-server"
    split_snapshot["authority"]["lifetime_authority_id"] = "split-authority"
    split_snapshot["authority"]["listener_id"] = "split-listener"
    split_snapshot["engine"]["engine_owner_id"] = "split-engine-owner"
    split_snapshot["engine"]["native_worker_id"] = "split-native-worker"
    expected_split_failures = {
        "one_engine_owner",
        "one_lifetime_authority",
        "one_listener",
        "one_model_load",
        "one_native_worker",
        "one_server",
    }
    try:
        derive_scenario_assertions(
            "cold_race",
            observations_by_kind=cold_transitions,
            process_observations=split_process_observations,
            invocations=[],
            same_account={
                "relation": "same_os_account",
                "plugin_hosts": [{"pid": 101}, {"pid": 102}],
            },
            materialization={},
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario cold_race raw evidence failed assertions: "
            + ", ".join(sorted(expected_split_failures)),
            "post-reset cold-race split changed its exact identity failures",
        )
    else:
        raise ProofFailure(
            "post-reset cold-race split escaped its exact identity assertions"
        )

def run_server_identity_self_tests(
    fixture: FullStackFixture,
) -> ServerIdentityFixture:
    valid = _engine_runtime_test(fixture)
    snapshot_payload, first_snapshot, shared = _shared_snapshot_test(fixture)
    process_observations, transitions = _cold_election_tests(first_snapshot)
    _cold_split_test(process_observations, transitions)
    return ServerIdentityFixture(
        snapshot_payload=snapshot_payload,
        first_snapshot=first_snapshot,
        shared=shared,
        valid_engine_identity=valid,
    )
