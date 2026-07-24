"""Qualification matrix and calibration self-tests."""

from __future__ import annotations

import argparse
import json

from .calibration_assembly import assemble_calibration_bundle
from .calibration_self_test import build_calibration_self_test_bundle
from .calibration_verification import verify_calibration_bundle
from .contract_primitives import canonical_sha256, write_json
from .foundation import ProofFailure, require
from .measurement_samples import selected_qualification_matrix_cell
from .package_contracts import verify_package_server_contracts
from .qualification_artifacts import require_candidate_matrix_installation_source
from .self_test_full_stack_types import CalibrationFixture, FullStackFixture


def _qualification_matrix_tests(fixture: FullStackFixture) -> dict:
    manifest = fixture.manifest
    self_measurement_protocol = fixture.measurement_protocol
    measurement_contract = verify_package_server_contracts(
        manifest,
        self_measurement_protocol,
        require_frozen=False,
    )
    windows_candidate_cell_id = "candidate_installed_windows_x64_cpu"
    windows_candidate_cell = selected_qualification_matrix_cell(
        measurement_contract["measurement_protocol"],
        cell_id=windows_candidate_cell_id,
        target="windows-x64",
        proof_tier="installed_runtime",
        expected_policy="cpu_explicit",
        expected_backend="CPU",
    )
    require(
        windows_candidate_cell
        == {
            "asset_target": "windows-x64",
            "proof_tier": "installed_runtime",
            "host_class": "premerge_candidate_windows_x64",
            "policy": "cpu_explicit",
            "backend": "cpu",
            "cache_state": "reused",
            "residency_state": "resident",
            "accelerator_claim": "none",
        },
        "Windows candidate-installed alias changed its exact identity",
    )
    require_candidate_matrix_installation_source(
        windows_candidate_cell_id,
        "candidate",
    )
    try:
        require_candidate_matrix_installation_source(
            windows_candidate_cell_id,
            "marketplace",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "Windows candidate-installed alias accepted marketplace provenance"
        )
    hostile_windows_alias_values = {
        "asset_target": "linux-x64",
        "proof_tier": "protected_hardware",
        "policy": "accelerated",
        "backend": "vulkan",
        "accelerator_claim": "vulkan",
    }
    for field, hostile_value in hostile_windows_alias_values.items():
        hostile_protocol = json.loads(
            json.dumps(measurement_contract["measurement_protocol"])
        )
        hostile_protocol["host_package_matrix"]["installed_windows_x64_cpu"][field] = (
            hostile_value
        )
        try:
            selected_qualification_matrix_cell(
                hostile_protocol,
                cell_id=windows_candidate_cell_id,
                target="windows-x64",
                proof_tier="installed_runtime",
                expected_policy="cpu_explicit",
                expected_backend="CPU",
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"Windows candidate-installed alias accepted changed {field}"
            )
    return measurement_contract


def _calibration_bundle_tests(
    fixture: FullStackFixture,
    measurement_contract: dict,
) -> CalibrationFixture:
    root = fixture.root
    manifest = fixture.manifest
    self_measurement_protocol = fixture.measurement_protocol
    (
        calibration_bundle_path,
        frozen_measurement_contract,
        calibration_bundle_payload,
    ) = build_calibration_self_test_bundle(
        root,
        measurement_contract,
        source=manifest["source"],
    )
    assembled_run_paths = []
    for index, run in enumerate(calibration_bundle_payload["runs"]):
        run_path = root / "assembler-runs" / f"run-{index + 1}.json"
        write_json(run_path, run)
        assembled_run_paths.append(run_path)
    assembled_bundle_path = root / "assembled-calibration-bundle.json"
    assembled_constant_path = root / "assembled-constant-set.json"
    assembled = assemble_calibration_bundle(
        argparse.Namespace(
            measurement_protocol=self_measurement_protocol,
            calibration_bundle_output=assembled_bundle_path,
            frozen_constant_set_output=assembled_constant_path,
            freeze_selected_at="self-test",
            calibration_run=assembled_run_paths,
            calibration_producer_repository="TheGreenCedar/CodeStory",
            calibration_producer_workflow_path=(
                ".github/workflows/packaged-platform-pr.yml"
            ),
            calibration_producer_run_id="123",
            calibration_producer_run_attempt="1",
            calibration_producer_artifact=(
                f"embedding-calibration-bundle-{manifest['source']['commit']}"
            ),
        )
    )
    require(
        assembled["run_count"] == 6
        and assembled["matrix_cell_count"] == 2
        and assembled_bundle_path.is_file()
        and assembled_constant_path.is_file(),
        "calibration assembler did not produce the exact frozen artifacts",
    )
    calibration_result = verify_calibration_bundle(
        calibration_bundle_path,
        frozen_measurement_contract,
        enforce_source_lineage=False,
    )
    require(
        calibration_result["run_count"] == 6
        and calibration_result["matrix_cell_count"] == 2,
        "calibration bundle self-test did not verify the full matrix",
    )
    return CalibrationFixture(
        bundle_path=calibration_bundle_path,
        bundle_payload=calibration_bundle_payload,
        frozen_measurement_contract=frozen_measurement_contract,
    )


def _calibration_hostile_tests(
    fixture: FullStackFixture,
    calibration: CalibrationFixture,
) -> None:
    root = fixture.root
    calibration_bundle_path = calibration.bundle_path
    calibration_bundle_payload = calibration.bundle_payload
    frozen_measurement_contract = calibration.frozen_measurement_contract
    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    hostile_calibration["runs"].pop()
    hostile_calibration_path = root / "hostile-calibration-bundle.json"
    write_json(hostile_calibration_path, hostile_calibration)
    try:
        verify_calibration_bundle(
            hostile_calibration_path,
            frozen_measurement_contract,
            enforce_source_lineage=False,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("incomplete calibration matrix was accepted")
    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    hostile_run = hostile_calibration["runs"][0]
    hostile_metric = hostile_run["raw_artifact"]["payload"]["metrics"][
        "cold_first_vector"
    ]
    hostile_metric["samples"][0]["operands"].pop("successful_operation_duration_ns")
    hostile_run["raw_artifact"]["sha256"] = canonical_sha256(
        hostile_run["raw_artifact"]["payload"]
    )
    write_json(hostile_calibration_path, hostile_calibration)
    try:
        verify_calibration_bundle(
            hostile_calibration_path,
            frozen_measurement_contract,
            enforce_source_lineage=False,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "calibration sample without successful operation duration was accepted"
        )
    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    first_sample_id = hostile_calibration["runs"][0]["raw_artifact"]["payload"][
        "metrics"
    ]["warm_query_ipc"]["samples"][0]["sample_id"]
    duplicate_run = hostile_calibration["runs"][1]
    duplicate_run["raw_artifact"]["payload"]["metrics"]["warm_query_ipc"]["samples"][0][
        "sample_id"
    ] = first_sample_id
    duplicate_run["raw_artifact"]["sha256"] = canonical_sha256(
        duplicate_run["raw_artifact"]["payload"]
    )
    write_json(hostile_calibration_path, hostile_calibration)
    try:
        verify_calibration_bundle(
            hostile_calibration_path,
            frozen_measurement_contract,
            enforce_source_lineage=False,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("duplicate calibration sample identity was accepted")
    hostile_frozen_contract = json.loads(json.dumps(frozen_measurement_contract))
    hostile_frozen_contract["constant_set"]["qualification_thresholds"][
        "warm_query_ipc"
    ] += 1
    try:
        verify_calibration_bundle(
            calibration_bundle_path,
            hostile_frozen_contract,
            enforce_source_lineage=False,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("post-result calibration threshold change was accepted")


def run_calibration_self_tests(fixture: FullStackFixture) -> dict:
    measurement_contract = _qualification_matrix_tests(fixture)
    calibration = _calibration_bundle_tests(fixture, measurement_contract)
    _calibration_hostile_tests(fixture, calibration)
    return measurement_contract
