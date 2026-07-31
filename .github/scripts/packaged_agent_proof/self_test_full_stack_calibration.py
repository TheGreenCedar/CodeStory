"""Qualification matrix and calibration self-tests."""

from __future__ import annotations

import argparse
import json

from .calibration_assembly import assemble_calibration_bundle
from .calibration_self_test import (
    SelfTestRunContext,
    _self_test_sample,
    build_calibration_self_test_bundle,
)
from .calibration_verification import verify_calibration_bundle
from .constant_calibration import _validate_driver_output
from .contract_primitives import canonical_sha256, sha256, write_json
from .foundation import TARGET_CONTRACTS, ProofFailure, require
from .measurement_protocol import load_measurement_protocol
from .measurement_samples import selected_qualification_matrix_cell
from .package_contracts import verify_package_server_contracts
from .qualification_measurements import (
    MeasurementValidationContract,
    _qualification_measurement_sample,
)
from .self_test_full_stack_types import CalibrationFixture, FullStackFixture


def _qualification_matrix_tests(fixture: FullStackFixture) -> dict:
    manifest = fixture.manifest
    self_measurement_protocol = fixture.measurement_protocol
    measurement_contract = verify_package_server_contracts(
        manifest,
        self_measurement_protocol,
        require_frozen=False,
    )
    for label, field, value in (
        (
            "server idle epoch",
            "phase",
            [
                "last_queued_active_or_leased_work_ended",
                "engine_and_server_absent",
            ],
        ),
        (
            "pre-completion workload",
            "workload",
            "true_idle_60000_awake_ms_v1",
        ),
    ):
        regressed_true_idle = json.loads(
            json.dumps(measurement_contract["measurement_protocol"])
        )
        if field == "phase":
            regressed_true_idle["phase_boundaries"]["true_idle_exit"] = value
        else:
            regressed_true_idle["workloads"]["true_idle_exit"]["workload_id"] = value
        regressed_true_idle_path = (
            fixture.root / f"true-idle-{field}-regression.json"
        )
        write_json(regressed_true_idle_path, regressed_true_idle)
        try:
            load_measurement_protocol(regressed_true_idle_path)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(f"true-idle qualification accepted {label}")
    for quality_metric in (
        "answer_quality",
        "packet_quality",
        "publishable_packet_pass_rate",
    ):
        quality_reintroduced = json.loads(
            json.dumps(measurement_contract["measurement_protocol"])
        )
        quality_reintroduced["required_metrics"].append(quality_metric)
        quality_reintroduced["phase_boundaries"][quality_metric] = [
            "publishable_packet_candidate_fixed",
            "publishable_packet_pass_rate_scored",
        ]
        quality_reintroduced["workloads"][quality_metric] = {
            "workload_id": "publishable_three_repeat_packet_v1",
            "owner_state": "external_exact_head_artifact",
            "operation": "packet_runtime",
            "input_generator": "axios_js_ts_v2",
        }
        quality_reintroduced["metric_sampling"][quality_metric] = {
            "sample_count": 3,
            "aggregation": "minimum",
        }
        quality_reintroduced["metric_contracts"][quality_metric] = {
            "comparison": "greater_than_or_equal",
            "unit": "publishable_packet_pass_rate",
        }
        quality_protocol_path = (
            fixture.root / f"{quality_metric}-reintroduced-protocol.json"
        )
        write_json(quality_protocol_path, quality_reintroduced)
        try:
            load_measurement_protocol(quality_protocol_path)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"shape-complete {quality_metric} re-entered frozen-candidate qualification"
            )
    for quality_assertion in (
        "answer_quality_sufficient",
        "packet_quality_pass",
        "publishable_packet_pass_rate_is_one",
    ):
        quality_reintroduced = json.loads(
            json.dumps(measurement_contract["measurement_protocol"])
        )
        quality_reintroduced["scenario_contracts"]["frozen_owner"][
            "required"
        ].append(quality_assertion)
        quality_protocol_path = (
            fixture.root / f"{quality_assertion}-scenario-protocol.json"
        )
        write_json(quality_protocol_path, quality_reintroduced)
        try:
            load_measurement_protocol(quality_protocol_path)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"{quality_assertion} re-entered lifecycle scenario qualification"
            )
    repurposed_metric = json.loads(
        json.dumps(measurement_contract["measurement_protocol"])
    )
    repurposed_metric["phase_boundaries"]["warm_query_ipc"] = [
        "publishable_packet_candidate_fixed",
        "publishable_packet_pass_rate_scored",
    ]
    repurposed_metric["calibration_phase_boundaries"]["warm_query_ipc"] = list(
        repurposed_metric["phase_boundaries"]["warm_query_ipc"]
    )
    repurposed_metric["workloads"]["warm_query_ipc"] = {
        "workload_id": "publishable_three_repeat_packet_v1",
        "owner_state": "external_exact_head_artifact",
        "operation": "packet_runtime",
        "input_generator": "axios_js_ts_v2",
    }
    repurposed_metric["metric_sampling"]["warm_query_ipc"] = {
        "sample_count": 3,
        "aggregation": "minimum",
    }
    repurposed_metric["metric_contracts"]["warm_query_ipc"] = {
        "comparison": "greater_than_or_equal",
        "unit": "publishable_packet_pass_rate",
    }
    repurposed_protocol_path = fixture.root / "repurposed-quality-metric.json"
    write_json(repurposed_protocol_path, repurposed_metric)
    try:
        load_measurement_protocol(repurposed_protocol_path)
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "warm query metric was repurposed as packet quality"
        )
    windows_cell_id = "protected_windows_x64_vulkan"
    windows_cell = selected_qualification_matrix_cell(
        measurement_contract["measurement_protocol"],
        cell_id=windows_cell_id,
        target="windows-x64",
        proof_tier="protected_hardware",
        expected_policy="accelerated",
        expected_backend="Vulkan",
    )
    require(
        windows_cell
        == {
            "asset_target": "windows-x64",
            "proof_tier": "protected_hardware",
            "host_class": "protected_self_hosted_windows_x64",
            "policy": "accelerated",
            "backend": "vulkan",
            "cache_state": "reused",
            "residency_state": "resident",
            "accelerator_claim": "vulkan",
        },
        "protected Windows cell changed its exact identity",
    )
    hostile_windows_values = {
        "asset_target": "linux-x64",
        "proof_tier": "installed_runtime",
        "policy": "cpu_explicit",
        "backend": "cpu",
    }
    for field, hostile_value in hostile_windows_values.items():
        hostile_protocol = json.loads(
            json.dumps(measurement_contract["measurement_protocol"])
        )
        hostile_protocol["host_package_matrix"][windows_cell_id][field] = (
            hostile_value
        )
        try:
            selected_qualification_matrix_cell(
                hostile_protocol,
                cell_id=windows_cell_id,
                target="windows-x64",
                proof_tier="protected_hardware",
                expected_policy="accelerated",
                expected_backend="Vulkan",
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"protected Windows cell accepted changed {field}"
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
        assembled["run_count"] == 3
        and assembled["matrix_cell_count"] == 1
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
        calibration_result["run_count"] == 3
        and calibration_result["matrix_cell_count"] == 1,
        "calibration bundle self-test did not verify the full matrix",
    )
    require(
        calibration_result["source_lineage"] is None,
        "an unenforced verification reported a calibration source lineage",
    )
    # The flag must not be inert: with lineage enforcement on and no packaged
    # source to bind, the freeze has to refuse rather than silently skip the
    # guard the release workflow now depends on.
    try:
        verify_calibration_bundle(
            calibration_bundle_path,
            frozen_measurement_contract,
            enforce_source_lineage=True,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "enforced calibration source lineage was skipped without a packaged source"
        )
    return CalibrationFixture(
        bundle_path=calibration_bundle_path,
        bundle_payload=calibration_bundle_payload,
        frozen_measurement_contract=frozen_measurement_contract,
    )


def _measurement_window_semantics_tests(
    fixture: FullStackFixture,
    calibration: CalibrationFixture,
) -> None:
    """The declared measurement windows are the checker's contract.

    Regression for calibration run 30205779627: the per-sample worker driver
    stamped every metric with the generic whole-worker phase pair
    ``packaged_worker_operation_started`` ->
    ``packaged_worker_operation_validated`` and three non-protocol workload
    ids. This drives the retained-sample validator directly so nothing else
    (digest lineage, freeze records) can mask the two gates: a sample labeled
    with the protocol's declared phase boundaries and workload id passes,
    while the generic pair and the regressed workload ids stay rejected.
    """
    del fixture
    protocol = calibration.frozen_measurement_contract["measurement_protocol"]
    cell_id, cell = sorted(protocol["calibration_matrix"].items())[0]
    target_os = TARGET_CONTRACTS[cell["asset_target"]]["target_os"]
    awake_api = protocol["clock_policy"]["platform_apis"][target_os][0]
    inclusive_api = protocol["clock_policy"]["suspend_detection"]["platform_apis"][
        target_os
    ]
    raw_metric_names = frozenset(protocol["calibration_required_metrics"])
    validation = MeasurementValidationContract(
        contracts={},
        protocol=protocol,
        metric_contracts=protocol["metric_contracts"],
        phase_boundaries=protocol["calibration_phase_boundaries"],
        matrix_cell_id=cell_id,
        matrix_cell=cell,
        expected_policy=cell["policy"],
        expected_backend=cell["backend"],
        raw_metric_names=raw_metric_names,
        allowed_awake_apis=frozenset({awake_api}),
        inclusive_api=inclusive_api,
        maximum_suspend_ns=protocol["clock_policy"]["suspend_detection"][
            "maximum_inclusive_minus_awake_ns"
        ],
    )
    context = SelfTestRunContext(
        protocol,
        {},
        {},
        cell_id,
        cell,
        0,
        1,
        awake_api,
        inclusive_api,
    )
    regressed_workloads = {
        "spawn_convergence": "compatible_query_absent_owner_v1",
        "existing_owner_connect": "observe_existing_owner_v1",
        "busy_retry_usefulness": "held_query_release_v1",
    }
    for position, metric in enumerate(sorted(raw_metric_names)):
        declared = _self_test_sample(context, metric, position, 1)
        _qualification_measurement_sample(
            metric,
            declared,
            sample_index=0,
            validation=validation,
        )
        generic = json.loads(json.dumps(declared))
        generic["start"]["phase"] = "packaged_worker_operation_started"
        generic["end"]["phase"] = "packaged_worker_operation_validated"
        try:
            _qualification_measurement_sample(
                metric,
                generic,
                sample_index=0,
                validation=validation,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"generic whole-worker measurement phases were accepted for {metric}"
            )
        if metric in regressed_workloads:
            wrong_workload = json.loads(json.dumps(declared))
            wrong_workload["workload_id"] = regressed_workloads[metric]
            try:
                _qualification_measurement_sample(
                    metric,
                    wrong_workload,
                    sample_index=0,
                    validation=validation,
                )
            except ProofFailure:
                pass
            else:
                raise ProofFailure(
                    f"non-protocol workload id was accepted for {metric}"
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

    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    hostile_run = hostile_calibration["runs"][0]
    hostile_metric = hostile_run["raw_artifact"]["payload"]["metrics"][
        "warm_query_ipc"
    ]
    duplicate_sample = json.loads(json.dumps(hostile_metric["samples"][0]))
    duplicate_sample["repeat"] = 2
    duplicate_sample["sample_id"] += "-repeat"
    hostile_metric["samples"].append(duplicate_sample)
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
        raise ProofFailure("three-by-three calibration sampling was accepted")

    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    hostile_run = hostile_calibration["runs"][0]
    source_sample = hostile_run["raw_artifact"]["payload"]["metrics"][
        "warm_query_ipc"
    ]["samples"][0]
    qualification_metric = json.loads(json.dumps(source_sample))
    qualification_metric["sample_id"] += "-true-idle"
    qualification_metric["workload_id"] = "true-idle-qualification"
    hostile_run["raw_artifact"]["payload"]["metrics"]["true_idle_exit"] = {
        "unit": "milliseconds",
        "samples": [qualification_metric],
    }
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
        raise ProofFailure("qualification-only metric was accepted in calibration")

    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    first_run = hostile_calibration["runs"][0]
    second_run = hostile_calibration["runs"][1]
    first_metrics = first_run["raw_artifact"]["payload"]["metrics"]
    second_metrics = second_run["raw_artifact"]["payload"]["metrics"]
    for metric in second_metrics:
        second_metrics[metric]["samples"][0]["server_identity"] = json.loads(
            json.dumps(first_metrics[metric]["samples"][0]["server_identity"])
        )
    second_run["raw_artifact"]["sha256"] = canonical_sha256(
        second_run["raw_artifact"]["payload"]
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
        raise ProofFailure("calibration reused a server generation across clean runs")

    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    hostile_run = hostile_calibration["runs"][1]
    hostile_run["materialized_reused"] = False
    hostile_run["raw_artifact"]["payload"]["materialized_reused"] = False
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
        raise ProofFailure("calibration repeated model materialization after run one")

    hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
    hostile_calibration["qualification_thresholds"] = {
        "warm_query_ipc": 1,
    }
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
        raise ProofFailure("calibration bundle was allowed to select thresholds")


def _constant_collector_driver_tests(
    fixture: FullStackFixture,
    calibration: CalibrationFixture,
) -> None:
    protocol = calibration.frozen_measurement_contract["measurement_protocol"]
    measurement_contract = calibration.frozen_measurement_contract
    cell_id, cell = next(iter(protocol["calibration_matrix"].items()))
    private_root = fixture.root / "constant-driver"
    private_root.mkdir()
    package = calibration.bundle_payload["runs"][0]["package"]
    driver_package = {
        key: package[key]
        for key in (
            "archive_sha256",
            "executable_sha256",
            "asset_target",
            "release_version",
            "model_sha256",
        )
    }
    driver_contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "constant_set_sha256": measurement_contract["constant_set_sha256"],
        "measurement_protocol_sha256": measurement_contract[
            "measurement_protocol_sha256"
        ],
    }
    request = {
        "schema_version": 1,
        "source": fixture.manifest["source"],
        "package": driver_package,
        "contracts": driver_contracts,
        "runtime": {
            "engine_policy": "accelerated",
            "expected_backend": cell["backend"],
            "offline": True,
            "matrix_cell_id": cell_id,
            "cache_state": cell["cache_state"],
            "residency_state": cell["residency_state"],
        },
    }
    request_path = private_root / "request.json"
    write_json(request_path, request)
    summaries = []
    for run in calibration.bundle_payload["runs"]:
        run_index = run["run_index"]
        metrics = run["raw_artifact"]["payload"]["metrics"]
        identity = next(iter(metrics.values()))["samples"][0]["server_identity"]
        artifact_name = f"constant-calibration-run-{run_index}.raw.json"
        raw = {
            "schema_version": 1,
            "run_index": run_index,
            "contracts": driver_contracts,
            "metrics": metrics,
            "server_identities": [identity],
            "backend": cell["backend"],
            "policy": "accelerated",
            "model_sha256": package["model_sha256"],
            "materialized_reused": run_index > 1,
        }
        write_json(private_root / artifact_name, raw)
        summaries.append(
            {
                "run_index": run_index,
                "measurements": {
                    "artifact": artifact_name,
                    "metric_count": 9,
                    "sample_count": 9,
                },
                "server_identities": [identity],
                "backend": cell["backend"],
                "policy": "accelerated",
                "model_sha256": package["model_sha256"],
                "materialized_reused": run_index > 1,
            }
        )
    output = {
        "schema_version": 1,
        "source": request["source"],
        "package": driver_package,
        "contracts": driver_contracts,
        "runtime": request["runtime"],
        "request_sha256": sha256(request_path),
        "calibration_runs": summaries,
    }
    validated = _validate_driver_output(
        output,
        request=request,
        request_path=request_path,
        private_root=private_root,
        protocol=protocol,
        matrix_cell=cell,
    )
    require(
        len(validated) == 3,
        "constant-calibration driver output did not retain three clean runs",
    )

    hostile = json.loads(json.dumps(output))
    hostile["calibration_runs"][0]["backend"] = "cpu"
    try:
        _validate_driver_output(
            hostile,
            request=request,
            request_path=request_path,
            private_root=private_root,
            protocol=protocol,
            matrix_cell=cell,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("constant-calibration driver output accepted CPU")

    raw_path = private_root / "constant-calibration-run-1.raw.json"
    raw = json.loads(raw_path.read_text(encoding="utf-8"))
    raw["metrics"]["true_idle_exit"] = json.loads(
        json.dumps(raw["metrics"]["warm_query_ipc"])
    )
    write_json(raw_path, raw)
    try:
        _validate_driver_output(
            output,
            request=request,
            request_path=request_path,
            private_root=private_root,
            protocol=protocol,
            matrix_cell=cell,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "constant-calibration driver output accepted a qualification-only metric"
        )


def run_calibration_self_tests(fixture: FullStackFixture) -> dict:
    measurement_contract = _qualification_matrix_tests(fixture)
    calibration = _calibration_bundle_tests(fixture, measurement_contract)
    _measurement_window_semantics_tests(fixture, calibration)
    _calibration_hostile_tests(fixture, calibration)
    _constant_collector_driver_tests(fixture, calibration)
    return measurement_contract
