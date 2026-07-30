"""Measurement scenario, matrix, and sampling contract validation."""

from __future__ import annotations

import json
from pathlib import Path

from .contract_primitives import (
    canonical_sha256,
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
)
from .foundation import (
    LOWER_TIER_NONCLAIMS,
    QUALIFICATION_SCHEMA_VERSION,
    REQUIRED_QUALIFICATION_METRICS,
    REQUIRED_SERVER_SCENARIO_ASSERTIONS,
    REQUIRED_SERVER_SCENARIOS,
    ProofFailure,
    require,
)

QUALIFICATION_MEASUREMENT_SHAPE_FIELDS = (
    "required_scenarios",
    "scenario_contracts",
    "required_metrics",
    "phase_boundaries",
    "workloads",
    "metric_sampling",
    "metric_contracts",
    "calibration_required_metrics",
    "calibration_phase_boundaries",
    "calibration_metric_sampling",
    "calibration_workload_state_overrides",
)
EXPECTED_QUALIFICATION_MEASUREMENT_SHAPE_SHA256 = (
    "1c065562adc34d0d9978187857807e491c4e6d4aa233fdd94f5636931a7b730e"
)


def qualification_measurement_shape_sha256(protocol: dict) -> str:
    return canonical_sha256(
        {
            field: protocol.get(field)
            for field in QUALIFICATION_MEASUREMENT_SHAPE_FIELDS
        }
    )


def _measurement_document(path: Path) -> dict:
    require(path.is_file(), f"measurement protocol is missing: {path}")
    try:
        protocol = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"measurement protocol is not valid JSON: {exc}") from exc
    require(isinstance(protocol, dict), "measurement protocol must be an object")
    require(
        protocol.get("schema_version") == QUALIFICATION_SCHEMA_VERSION,
        "measurement protocol schema is unsupported",
    )
    return protocol


def _verify_scenario_and_metric_contracts(protocol: dict) -> tuple[set[str], dict]:
    require(
        qualification_measurement_shape_sha256(protocol)
        == EXPECTED_QUALIFICATION_MEASUREMENT_SHAPE_SHA256,
        "frozen-candidate qualification measurement shape changed",
    )
    require(
        set(protocol.get("required_scenarios", [])) == REQUIRED_SERVER_SCENARIOS,
        "measurement protocol does not name the complete server scenario set",
    )
    scenario_contracts = protocol.get("scenario_contracts")
    require(
        isinstance(scenario_contracts, dict)
        and set(scenario_contracts) == REQUIRED_SERVER_SCENARIOS,
        "measurement protocol scenario contracts do not match its required scenarios",
    )
    for scenario, contract in scenario_contracts.items():
        require(
            isinstance(contract, dict)
            and set(contract) == {"required"}
            and isinstance(contract["required"], list)
            and bool(contract["required"])
            and len(set(contract["required"])) == len(contract["required"])
            and all(
                isinstance(assertion, str) and assertion
                for assertion in contract["required"]
            ),
            f"measurement scenario {scenario} assertion contract is malformed",
        )
        require(
            set(contract["required"]) == REQUIRED_SERVER_SCENARIO_ASSERTIONS[scenario],
            f"measurement scenario {scenario} assertion set changed",
        )
    require(
        set(protocol.get("required_lower_tier_nonclaims", [])) == LOWER_TIER_NONCLAIMS,
        "measurement protocol does not name the complete lower-tier nonclaim set",
    )
    required_metrics = set(protocol.get("required_metrics", []))
    require(
        required_metrics == REQUIRED_QUALIFICATION_METRICS,
        "frozen-candidate qualification metric set must remain lifecycle-only",
    )
    phase_boundaries = protocol.get("phase_boundaries")
    require(
        isinstance(phase_boundaries, dict)
        and set(phase_boundaries) == required_metrics,
        "measurement protocol phase boundaries do not match its required metrics",
    )
    for metric, boundaries in phase_boundaries.items():
        require(
            isinstance(boundaries, list)
            and len(boundaries) == 2
            and all(isinstance(event, str) and event for event in boundaries),
            f"measurement metric {metric} must have exact start and end events",
        )
    metric_contracts = protocol.get("metric_contracts")
    require(
        isinstance(metric_contracts, dict)
        and set(metric_contracts) == required_metrics,
        "measurement protocol metric contracts do not match its required metrics",
    )
    for metric, contract in metric_contracts.items():
        require(
            isinstance(contract, dict),
            f"measurement metric {metric} contract is malformed",
        )
        require(
            contract.get("comparison")
            in {"equal", "greater_than_or_equal", "less_than_or_equal"},
            f"measurement metric {metric} has an unsupported comparison",
        )
        require_nonempty_string(
            contract.get("unit"), f"measurement metric {metric} unit"
        )
    comparison_basis = protocol.get("comparison_basis")
    require(
        isinstance(comparison_basis, dict)
        and comparison_basis
        == {
            "type": "absolute_candidate_sla",
            "paired_incumbent_required": False,
            "warm_ipc_claim": "candidate_end_to_end_ipc_latency",
            "nonclaim": "overhead_relative_to_incumbent",
            "rationale": (
                "the incumbent in-process runtime does not expose the same server phase hooks "
                "or ownership semantics, so a paired delta would conflate IPC with runtime, "
                "cache, model-load, and lifecycle changes"
            ),
        },
        "measurement protocol must preregister absolute candidate SLAs and the incumbent-overhead nonclaim",
    )
    return required_metrics, metric_contracts


def _verify_host_package_matrix(matrix: dict) -> None:
    expected_cells = {
        "protected_macos_arm64_metal": (
            "macos-arm64",
            "protected_hardware",
            "protected_self_hosted_macos_arm64",
            "accelerated",
            "metal",
            "metal",
        ),
        "protected_windows_x64_vulkan": (
            "windows-x64",
            "protected_hardware",
            "protected_self_hosted_windows_x64",
            "accelerated",
            "vulkan",
            "vulkan",
        ),
        "protected_linux_x64_vulkan": (
            "linux-x64",
            "protected_hardware",
            "protected_self_hosted_linux_x64",
            "accelerated",
            "vulkan",
            "vulkan",
        ),
    }
    require(
        set(matrix) == set(expected_cells),
        "measurement host/package matrix does not match the release proof lanes",
    )
    for cell_id, cell in matrix.items():
        require_nonempty_string(cell_id, "measurement host/package matrix cell id")
        require(
            isinstance(cell, dict), f"measurement matrix cell {cell_id} is malformed"
        )
        require_exact_keys(
            cell,
            {
                "asset_target",
                "proof_tier",
                "host_class",
                "policy",
                "backend",
                "cache_state",
                "residency_state",
                "accelerator_claim",
            },
            f"measurement matrix cell {cell_id}",
        )
        require_nonempty_string(
            cell["host_class"], f"measurement matrix cell {cell_id}.host_class"
        )
        require(
            cell["cache_state"] == "reused" and cell["residency_state"] == "resident",
            f"measurement matrix cell {cell_id} changed cache or residency state",
        )
        observed = (
            cell["asset_target"],
            cell["proof_tier"],
            cell["host_class"],
            cell["policy"],
            cell["backend"],
            cell["accelerator_claim"],
        )
        require(
            observed == expected_cells[cell_id],
            f"measurement matrix cell {cell_id} does not match its release proof lane",
        )


def _verify_calibration_matrix(
    matrix: dict,
    calibration_matrix: object,
    optional_evidence_matrix: object,
) -> None:
    require(
        isinstance(calibration_matrix, dict)
        and set(calibration_matrix) == {"protected_macos_arm64_metal"},
        "measurement calibration matrix must contain only protected macOS Metal",
    )
    for cell_id, cell in calibration_matrix.items():
        require(
            isinstance(cell, dict)
            and set(cell)
            == {
                "asset_target",
                "proof_tier",
                "host_class",
                "policy",
                "backend",
                "cache_state",
                "residency_state",
                "accelerator_claim",
            }
            and cell["proof_tier"] == "calibration"
            and cell["cache_state"] == "reused"
            and cell["residency_state"] == "resident",
            f"measurement calibration matrix cell {cell_id} is malformed",
        )
        qualification_cell = matrix[cell_id]
        require(
            all(
                cell[field] == qualification_cell[field]
                for field in (
                    "asset_target",
                    "host_class",
                    "policy",
                    "backend",
                    "cache_state",
                    "residency_state",
                    "accelerator_claim",
                )
            ),
            f"measurement calibration matrix cell {cell_id} does not use its exact qualification path",
        )
    require(
        isinstance(optional_evidence_matrix, dict)
        and set(optional_evidence_matrix) == {"protected_linux_x64_vulkan"},
        "optional calibration evidence must contain only protected Linux Vulkan",
    )
    for cell_id, cell in optional_evidence_matrix.items():
        require(
            isinstance(cell, dict)
            and set(cell)
            == {
                "asset_target",
                "proof_tier",
                "host_class",
                "policy",
                "backend",
                "cache_state",
                "residency_state",
                "accelerator_claim",
                "feeds_constant_selection",
            }
            and cell["proof_tier"] == "calibration"
            and cell["cache_state"] == "reused"
            and cell["residency_state"] == "resident"
            and cell["feeds_constant_selection"] is False,
            f"optional calibration evidence cell {cell_id} is malformed",
        )
        qualification_cell = matrix[cell_id]
        require(
            all(
                cell[field] == qualification_cell[field]
                for field in (
                    "asset_target",
                    "host_class",
                    "policy",
                    "backend",
                    "cache_state",
                    "residency_state",
                    "accelerator_claim",
                )
            ),
            f"optional calibration evidence cell {cell_id} does not use its exact qualification path",
        )


def _verify_measurement_matrices(protocol: dict) -> None:
    matrix = protocol.get("host_package_matrix")
    require(
        isinstance(matrix, dict), "measurement protocol omitted its host/package matrix"
    )
    _verify_host_package_matrix(matrix)
    _verify_calibration_matrix(
        matrix,
        protocol.get("calibration_matrix"),
        protocol.get("optional_calibration_evidence_matrix"),
    )


def _verify_calibration_sampling(
    protocol: dict,
    required_metrics: set[str],
) -> None:
    expected_metrics = {
        "existing_owner_connect",
        "spawn_convergence",
        "cold_first_vector",
        "first_product_ready",
        "warm_query_ipc",
        "warm_bulk_ipc",
        "bulk_documents_per_second",
        "bulk_tokens_per_second",
        "busy_retry_usefulness",
    }
    calibration_metrics = protocol.get("calibration_required_metrics")
    require(
        isinstance(calibration_metrics, list)
        and len(calibration_metrics) == len(set(calibration_metrics))
        and set(calibration_metrics) == expected_metrics
        and set(calibration_metrics).issubset(required_metrics),
        "constant calibration must name exactly the nine runtime-constant source metrics",
    )
    sampling = protocol.get("calibration_metric_sampling")
    require(
        isinstance(sampling, dict) and set(sampling) == expected_metrics,
        "constant-calibration sample policy does not match its required metrics",
    )
    for metric, policy in sampling.items():
        require(
            isinstance(policy, dict)
            and policy == {"sample_count_per_run": 1},
            f"constant-calibration metric {metric} must take one sample per clean run",
        )
    boundaries = protocol.get("calibration_phase_boundaries")
    require(
        isinstance(boundaries, dict) and set(boundaries) == expected_metrics,
        "constant calibration must declare phase boundaries for exactly its nine metrics",
    )
    for metric, points in boundaries.items():
        require(
            isinstance(points, list)
            and len(points) == 2
            and all(isinstance(point, str) and point for point in points),
            f"constant-calibration metric {metric} has malformed phase boundaries",
        )
        if metric != "cold_first_vector":
            require(
                points == protocol["phase_boundaries"][metric],
                f"constant-calibration metric {metric} changed its shared phase boundary",
            )
    require(
        boundaries["cold_first_vector"]
        == [
            "product_request_started_with_fresh_owner_model_absent",
            "first_vector_and_engine_evidence_validated",
        ]
        and protocol.get("calibration_workload_state_overrides")
        == {"cold_first_vector": "fresh_owner_model_absent"},
        "constant calibration must measure cold-first-vector on the fresh owner before model materialization",
    )


def _verify_measurement_sampling(
    protocol: dict,
    required_metrics: set[str],
    metric_contracts: dict,
) -> None:
    workloads = protocol.get("workloads")
    require(
        isinstance(workloads, dict) and set(workloads) == required_metrics,
        "measurement workloads do not match required metrics",
    )
    for metric, workload in workloads.items():
        require(
            isinstance(workload, dict), f"measurement workload {metric} is malformed"
        )
        require_nonempty_string(
            workload.get("workload_id"), f"measurement workload {metric}.workload_id"
        )
        require_nonempty_string(
            workload.get("owner_state"), f"measurement workload {metric}.owner_state"
        )
        require_nonempty_string(
            workload.get("operation"), f"measurement workload {metric}.operation"
        )
        require_nonempty_string(
            workload.get("input_generator"),
            f"measurement workload {metric}.input_generator",
        )
    sampling = protocol.get("metric_sampling")
    require(
        isinstance(sampling, dict) and set(sampling) == required_metrics,
        "measurement sample policy does not match required metrics",
    )
    for metric, policy in sampling.items():
        require(
            isinstance(policy, dict), f"measurement sample policy {metric} is malformed"
        )
        count = require_positive_int(
            policy.get("sample_count"),
            f"measurement sample policy {metric}.sample_count",
        )
        aggregation = require_nonempty_string(
            policy.get("aggregation"),
            f"measurement sample policy {metric}.aggregation",
        )
        if metric in {
            "true_idle_exit",
            "backend_observed_accelerator_residency",
        }:
            require(
                count == 1 and bool(policy.get("single_sample_reason")),
                f"single-sample metric {metric} lacks its preregistered reason",
            )
        else:
            require(count == 3, f"measurement metric {metric} must use three repeats")
        expected_aggregation = {
            "less_than_or_equal": "maximum",
            "greater_than_or_equal": "minimum",
            "equal": "exact",
        }[metric_contracts[metric]["comparison"]]
        require(
            aggregation == expected_aggregation,
            f"measurement metric {metric} aggregation is not conservative",
        )
