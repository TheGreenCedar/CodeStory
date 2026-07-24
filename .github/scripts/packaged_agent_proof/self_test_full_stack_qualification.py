"""Retained installed-runtime qualification self-tests."""

from __future__ import annotations

import json

from .foundation import (
    LOWER_TIER_NONCLAIMS,
    PINNED_CODEX_CLI_VERSION,
    ProofFailure,
)
from .qualification_retained import verify_retained_qualification
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
        "executable_sha256": manifest["binary"]["sha256"],
        "asset_target": manifest["asset_target"],
        "release_version": manifest["release_version"],
        "model_sha256": manifest["model"]["sha256"],
        "matrix_cell_id": "installed_macos_arm64_cpu",
        "accelerator_claim": "none",
        "backend": "cpu",
        "policy": "cpu_explicit",
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
        "matrix_cell_id": "installed_macos_arm64_cpu",
        "host_class": "post_publish_macos_arm64",
        "accelerator_claim": "none",
        "backend": "cpu",
        "policy": "cpu_explicit",
        "cache_state": "reused",
        "residency_state": "resident",
        "unplanned_suspend": False,
    }
    return package, host


def _installation_evidence(fixture: FullStackFixture) -> tuple[dict, dict]:
    manifest = fixture.manifest
    installed_plugin = {
        "schema_version": 2,
        "installation_source": "codex_marketplace_install",
        "codex_cli_version": PINNED_CODEX_CLI_VERSION,
        "marketplace_repository": "TheGreenCedar/AgentPluginMarketplace",
        "marketplace_commit": "d" * 40,
        "plugin_id": "codestory",
        "plugin_version": "0.0.0",
        "plugin_source_commit": manifest["source"]["commit"],
        "plugin_package_sha256": "e" * 64,
    }
    managed_runtime = {
        "cli_source": "managed",
        "plugin_version": "0.0.0",
        "managed_binary_sha256": manifest["binary"]["sha256"],
        "archive_sha256": "b" * 64,
        "build_source": "github_release",
        "repo_ref": "v0.0.0",
        "provisioned_at": "self-test",
    }
    return installed_plugin, managed_runtime


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
    installed_plugin, managed_runtime = _installation_evidence(fixture)
    qualification_contract = json.loads(json.dumps(measurement_contract))
    qualification_contract["constant_set"]["qualification_thresholds"] = {
        metric: 1
        for metric in measurement_contract["measurement_protocol"]["required_metrics"]
    }
    retained = {
        "schema_version": 1,
        "status": "pass",
        "tier": "installed_runtime",
        "source": fixture.manifest["source"],
        "package": package,
        "host": host,
        "installed_plugin": installed_plugin,
        "managed_runtime": managed_runtime,
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
    retained["metrics"]["retrieval_quality"]["raw_evidence"] = external.quality
    for metric, result in retained["metrics"].items():
        if metric != "retrieval_quality":
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
        required_tier="installed_runtime",
        required_matrix_cell_id="installed_macos_arm64_cpu",
        expected_policy="cpu_explicit",
        expected_backend="cpu",
        expected_accelerator_claim="none",
        installed_plugin=candidate["installed_plugin"],
        managed_runtime=candidate["managed_runtime"],
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
    wrong_tier["tier"] = "protected_hardware"
    stale_shared = json.loads(json.dumps(retained))
    stale_shared["shared_identity"]["server_instance_id"] = "stale-server"
    wrong_cell = json.loads(json.dumps(retained))
    wrong_cell["package"]["matrix_cell_id"] = "protected_macos_arm64_metal"
    for candidate, message in (
        (missing_scenario, "incomplete installed scenario evidence was accepted"),
        (wrong_tier, "different-tier retained qualification was accepted"),
        (stale_shared, "stale retained shared server identity was accepted"),
        (wrong_cell, "wrong qualification matrix cell was accepted"),
    ):
        _expect_retained_rejected(
            candidate,
            fixture,
            server,
            qualification_contract,
            message,
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


def run_retained_qualification_self_tests(
    fixture: FullStackFixture,
    server: ServerIdentityFixture,
    external: ExternalEvidenceFixture,
    measurement_contract: dict,
) -> None:
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
    _engine_identity_hostiles(server)
