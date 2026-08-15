"""Retained qualification self-tests."""

from __future__ import annotations

import json
from dataclasses import replace

from .foundation import (
    LOWER_TIER_NONCLAIMS,
    ProofFailure,
    require,
)
from .native_manifest import runtime_executable_sha256
from .qualification_retained import verify_retained_qualification
from .qualification_thresholds import (
    WINDOWS_SPAWN_METRIC,
    WINDOWS_VULKAN_MATRIX_CELL,
    qualification_threshold_for,
    verify_qualification_threshold_contract,
)
from .self_test_full_stack_types import (
    ExternalEvidenceFixture,
    FullStackFixture,
    ServerIdentityFixture,
)
from .server_engine_identity import engine_identity


def _package_and_host_evidence(fixture: FullStackFixture) -> tuple[dict, dict]:
    manifest = fixture.manifest
    package = {
        "archive_sha256": "b" * 64,
        "executable_sha256": runtime_executable_sha256(manifest),
        "asset_target": manifest["asset_target"],
        "release_version": manifest["release_version"],
        "model_sha256": manifest["model"]["sha256"],
        "matrix_cell_id": "protected_macos_arm64_metal",
        "accelerator_claim": "metal",
        "backend": "metal",
        "policy": "accelerated",
        "cache_state": "reused",
        "residency_state": "resident",
        "protocol_sha256": fixture.protocol_sha256,
        "constant_set_sha256": fixture.constant_set_sha256,
        "measurement_protocol_sha256": fixture.measurement_protocol_sha256,
    }
    host = {
        "fingerprint": "f" * 64,
        "platform": "macos",
        "target": manifest["asset_target"],
        "matrix_cell_id": "protected_macos_arm64_metal",
        "host_class": "protected_self_hosted_macos_arm64",
        "accelerator_claim": "metal",
        "backend": "metal",
        "policy": "accelerated",
        "cache_state": "reused",
        "residency_state": "resident",
        "unplanned_suspend": False,
    }
    return package, host


def _scenario_evidence(measurement_contract: dict) -> dict:
    contracts = measurement_contract["measurement_protocol"]["scenario_contracts"]
    return {
        scenario_id: {
            "status": "pass",
            "assertions": {assertion: True for assertion in contract["required"]},
            "artifacts": [
                {
                    "name": f"{scenario_id}.json",
                    "sha256": "c" * 64,
                }
            ],
        }
        for scenario_id, contract in contracts.items()
    }


def _metric_evidence(measurement_contract: dict) -> dict:
    protocol = measurement_contract["measurement_protocol"]
    return {
        metric: {
            "status": "pass",
            "unit": protocol["metric_contracts"][metric]["unit"],
            "value": 1,
            "threshold": 1,
            "comparison": protocol["metric_contracts"][metric]["comparison"],
        }
        for metric in protocol["required_metrics"]
    }


def _build_retained_evidence(
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    external: ExternalEvidenceFixture,
    measurement_contract: dict,
) -> tuple[dict, dict]:
    package, host = _package_and_host_evidence(fixture)
    qualification_contract = json.loads(json.dumps(measurement_contract))
    qualification_contract["constant_set"]["qualification_thresholds"] = {
        metric: 1
        for metric in measurement_contract["measurement_protocol"]["required_metrics"]
    }
    retained = {
        "schema_version": 1,
        "status": "pass",
        "tier": "protected_hardware",
        "source": fixture.manifest["source"],
        "package": package,
        "host": host,
        "same_account": {
            "account_id": "uid:501",
            "relation": "same_os_account",
            "cross_login_or_terminal_sessions_proven": False,
            "plugin_hosts": [
                {
                    "pid": 201,
                    "process_start_id": "boot-1:201",
                    "repository_id": "repo:a",
                },
                {
                    "pid": 202,
                    "process_start_id": "boot-1:202",
                    "repository_id": "repo:b",
                },
            ],
        },
        "shared_identity": server.shared,
        "timing": {
            "clock_domain": "awake_monotonic",
            "cross_process_timestamp_subtraction": False,
            "unplanned_suspend": False,
            "constants_frozen_before_run": True,
            "constant_set_sha256": fixture.constant_set_sha256,
        },
        "scenarios": _scenario_evidence(measurement_contract),
        "lower_tier_nonclaims": {
            claim: {
                "claimed": False,
                "reason": "self-test lower-tier boundary",
            }
            for claim in LOWER_TIER_NONCLAIMS
        },
        "metrics": _metric_evidence(measurement_contract),
    }
    retained["scenarios"]["server_crash"]["artifacts"].extend(
        [external.publication["artifact"], external.consistency["artifact"]]
    )
    retained["scenarios"]["worker_stall"]["artifacts"].append(
        external.publication["artifact"]
    )
    for metric, result in retained["metrics"].items():
        result["raw_evidence"] = {
            "name": (
                "total-codestory-process-memory.raw.json"
                if metric == "total_codestory_process_memory"
                else "measurements.raw.json"
            ),
            "sha256": "d" * 64,
        }
    return retained, qualification_contract


def _verify_retained(
    candidate: dict,
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    qualification_contract: dict,
) -> None:
    verify_retained_qualification(
        candidate,
        manifest=fixture.manifest,
        archive_sha256="b" * 64,
        shared_identity=server.shared,
        measurement_contract=qualification_contract,
        required_tier="protected_hardware",
        required_matrix_cell_id="protected_macos_arm64_metal",
        expected_policy="accelerated",
        expected_backend="metal",
        expected_accelerator_claim="metal",
        installed_plugin=None,
        managed_runtime=None,
    )


def _expect_retained_rejected(
    candidate: dict,
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    qualification_contract: dict,
    message: str,
) -> None:
    try:
        _verify_retained(candidate, fixture, server, qualification_contract)
    except ProofFailure:
        pass
    else:
        raise ProofFailure(message)


def _retained_hostile_tests(
    retained: dict,
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    qualification_contract: dict,
) -> None:
    missing_scenario = json.loads(json.dumps(retained))
    missing_scenario["scenarios"].pop("frozen_owner")
    wrong_tier = json.loads(json.dumps(retained))
    wrong_tier["tier"] = "installed_runtime"
    stale_shared = json.loads(json.dumps(retained))
    stale_shared["shared_identity"]["server_instance_id"] = "stale-server"
    wrong_cell = json.loads(json.dumps(retained))
    wrong_cell["package"]["matrix_cell_id"] = "hosted_linux_x64_cpu"
    wrong_cell_threshold = json.loads(json.dumps(retained))
    windows_spawn_threshold = qualification_contract["constant_set"][
        "qualification_threshold_overrides"
    ][WINDOWS_VULKAN_MATRIX_CELL][WINDOWS_SPAWN_METRIC]
    wrong_cell_threshold["metrics"][WINDOWS_SPAWN_METRIC][
        "threshold"
    ] = windows_spawn_threshold
    extra_quality_metric = json.loads(json.dumps(retained))
    extra_quality_metric["metrics"]["packet_quality"] = {
        "status": "pass",
        "unit": "ratio",
        "value": 1,
        "threshold": 1,
        "comparison": "greater_than_or_equal",
        "raw_evidence": {
            "name": "measurements.raw.json",
            "sha256": "d" * 64,
        },
    }
    for candidate, message in (
        (missing_scenario, "incomplete scenario evidence was accepted"),
        (wrong_tier, "different-tier retained qualification was accepted"),
        (stale_shared, "stale retained shared server identity was accepted"),
        (wrong_cell, "wrong qualification matrix cell was accepted"),
        (
            wrong_cell_threshold,
            "a Windows-only threshold override was accepted for the macOS matrix cell",
        ),
        (
            extra_quality_metric,
            "optional retrieval quality re-entered frozen-candidate qualification",
        ),
    ):
        _expect_retained_rejected(
            candidate,
            fixture,
            server,
            qualification_contract,
            message,
        )
    quality_contract_reintroduced = json.loads(json.dumps(qualification_contract))
    quality_contract_reintroduced["measurement_protocol"]["required_metrics"].append(
        "publishable_packet_pass_rate"
    )
    quality_contract_reintroduced["measurement_protocol"]["metric_contracts"][
        "publishable_packet_pass_rate"
    ] = {
        "comparison": "greater_than_or_equal",
        "unit": "ratio",
    }
    quality_contract_reintroduced["constant_set"]["qualification_thresholds"][
        "publishable_packet_pass_rate"
    ] = 1
    coherent_quality_metric = json.loads(json.dumps(retained))
    coherent_quality_metric["metrics"]["publishable_packet_pass_rate"] = (
        extra_quality_metric["metrics"]["packet_quality"]
    )
    _expect_retained_rejected(
        coherent_quality_metric,
        fixture,
        server,
        quality_contract_reintroduced,
        "shape-complete retrieval quality re-entered retained qualification",
    )
    quality_assertion_contract = json.loads(json.dumps(qualification_contract))
    quality_assertion_contract["measurement_protocol"]["scenario_contracts"][
        "frozen_owner"
    ]["required"].append("packet_quality_pass")
    quality_assertion_evidence = json.loads(json.dumps(retained))
    quality_assertion_evidence["scenarios"]["frozen_owner"]["assertions"][
        "packet_quality_pass"
    ] = True
    _expect_retained_rejected(
        quality_assertion_evidence,
        fixture,
        server,
        quality_assertion_contract,
        "packet quality re-entered retained lifecycle assertions",
    )
    repurposed_metric_contract = json.loads(json.dumps(qualification_contract))
    repurposed_protocol = repurposed_metric_contract["measurement_protocol"]
    repurposed_protocol["phase_boundaries"]["warm_query_ipc"] = [
        "publishable_packet_candidate_fixed",
        "publishable_packet_pass_rate_scored",
    ]
    repurposed_protocol["calibration_phase_boundaries"]["warm_query_ipc"] = list(
        repurposed_protocol["phase_boundaries"]["warm_query_ipc"]
    )
    repurposed_protocol["workloads"]["warm_query_ipc"] = {
        "workload_id": "publishable_three_repeat_packet_v1",
        "owner_state": "external_exact_head_artifact",
        "operation": "packet_runtime",
        "input_generator": "axios_js_ts_v2",
    }
    repurposed_protocol["metric_sampling"]["warm_query_ipc"] = {
        "sample_count": 3,
        "aggregation": "minimum",
    }
    repurposed_protocol["metric_contracts"]["warm_query_ipc"] = {
        "comparison": "greater_than_or_equal",
        "unit": "publishable_packet_pass_rate",
    }
    repurposed_metric_evidence = json.loads(json.dumps(retained))
    repurposed_metric_evidence["metrics"]["warm_query_ipc"].update(
        {
            "unit": "publishable_packet_pass_rate",
            "comparison": "greater_than_or_equal",
        }
    )
    _expect_retained_rejected(
        repurposed_metric_evidence,
        fixture,
        server,
        repurposed_metric_contract,
        "warm query retained metric was repurposed as packet quality",
    )


def _engine_identity_hostiles(server: ServerIdentityFixture) -> None:
    valid = server.valid_engine_identity
    invalid = {**valid, "embedding_adapter": "llvmpipe"}
    try:
        engine_identity(invalid, "accelerated", "Metal")
    except ProofFailure:
        pass
    else:
        raise ProofFailure("software adapter was accepted")
    inferred = {
        **valid,
        "embedding_execution_observation_source": "inferred_from_request",
    }
    try:
        engine_identity(inferred, "accelerated", "Metal")
    except ProofFailure:
        pass
    else:
        raise ProofFailure("inferred accelerator execution was accepted")


def _split_executable_retained_test(
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    external: ExternalEvidenceFixture,
    measurement_contract: dict,
) -> None:
    manifest = json.loads(json.dumps(fixture.manifest))
    runtime_digest = "e" * 64
    manifest["runtime_executable"]["sha256"] = runtime_digest
    split_fixture = replace(fixture, manifest=manifest)
    retained, qualification_contract = _build_retained_evidence(
        split_fixture,
        server,
        external,
        measurement_contract,
    )
    require(
        retained["package"]["executable_sha256"] == runtime_digest,
        "retained qualification used the launcher digest for a split runtime package",
    )
    _verify_retained(
        retained,
        split_fixture,
        server,
        qualification_contract,
    )
    launcher_bound = json.loads(json.dumps(retained))
    launcher_bound["package"]["executable_sha256"] = manifest["binary"]["sha256"]
    _expect_retained_rejected(
        launcher_bound,
        split_fixture,
        server,
        qualification_contract,
        "retained qualification accepted the launcher digest as the runtime executable",
    )


def run_retained_qualification_self_tests(
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    external: ExternalEvidenceFixture,
    measurement_contract: dict,
) -> None:
    protocol = measurement_contract["measurement_protocol"]
    constant_set = measurement_contract["constant_set"]
    thresholds = constant_set["qualification_thresholds"]
    verify_qualification_threshold_contract(
        constant_set,
        set(protocol["required_metrics"]),
    )
    windows_spawn_threshold = constant_set["qualification_threshold_overrides"][
        WINDOWS_VULKAN_MATRIX_CELL
    ][WINDOWS_SPAWN_METRIC]
    require(
        qualification_threshold_for(
            constant_set,
            WINDOWS_SPAWN_METRIC,
            "protected_macos_arm64_metal",
        )
        == thresholds[WINDOWS_SPAWN_METRIC]
        and qualification_threshold_for(
            constant_set,
            WINDOWS_SPAWN_METRIC,
            WINDOWS_VULKAN_MATRIX_CELL,
        )
        == windows_spawn_threshold,
        "qualification threshold selection did not preserve its matrix boundary",
    )
    mismatched_override = json.loads(json.dumps(constant_set))
    mismatched_override["qualification_threshold_overrides"][
        WINDOWS_VULKAN_MATRIX_CELL
    ][WINDOWS_SPAWN_METRIC] += 1
    try:
        verify_qualification_threshold_contract(
            mismatched_override,
            set(protocol["required_metrics"]),
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "a Windows spawn threshold detached from the selected slow-host bound was accepted"
        )
    require(
        set(protocol["required_metrics"])
        == {
            "backend_observed_accelerator_residency",
            "bulk_documents_per_second",
            "bulk_tokens_per_second",
            "busy_retry_usefulness",
            "cold_first_vector",
            "existing_owner_connect",
            "first_product_ready",
            "spawn_convergence",
            "total_codestory_process_memory",
            "true_idle_exit",
            "warm_bulk_ipc",
            "warm_query_ipc",
        }
        and set(thresholds) == set(protocol["required_metrics"]),
        "frozen-candidate qualification metric set changed",
    )
    retained, qualification_contract = _build_retained_evidence(
        fixture,
        server,
        external,
        measurement_contract,
    )
    _verify_retained(retained, fixture, server, qualification_contract)
    _retained_hostile_tests(
        retained,
        fixture,
        server,
        qualification_contract,
    )
    _split_executable_retained_test(
        fixture,
        server,
        external,
        measurement_contract,
    )
    _engine_identity_hostiles(server)
