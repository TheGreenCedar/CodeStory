"""Measurement scenario, matrix, and sampling contract validation."""

from __future__ import annotations

import json
from pathlib import Path

from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
)
from .foundation import (
    LOWER_TIER_NONCLAIMS,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    QUALIFICATION_SCHEMA_VERSION,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    ProofFailure,
    require,
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
        set(protocol.get("required_lower_tier_nonclaims", [])) == LOWER_TIER_NONCLAIMS,
        "measurement protocol does not name the complete lower-tier nonclaim set",
    )
    required_metrics = set(protocol.get("required_metrics", []))
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
    expected_matrix = {
        ("linux-x64", "hosted_package", "cpu_explicit", "cpu", "none"),
        ("macos-arm64", "protected_hardware", "accelerated", "metal", "metal"),
        ("windows-x64", "protected_hardware", "accelerated", "vulkan", "vulkan"),
        ("linux-x64", "installed_runtime", "cpu_explicit", "cpu", "none"),
        ("linux-arm64", "installed_runtime", "cpu_explicit", "cpu", "none"),
        ("macos-x64", "installed_runtime", "cpu_explicit", "cpu", "none"),
        ("macos-arm64", "installed_runtime", "cpu_explicit", "cpu", "none"),
        ("windows-x64", "installed_runtime", "cpu_explicit", "cpu", "none"),
        ("windows-arm64", "installed_runtime", "cpu_explicit", "cpu", "none"),
    }
    observed_matrix: set[tuple[str, str, str, str, str]] = set()
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
        observed_matrix.add(
            (
                cell["asset_target"],
                cell["proof_tier"],
                cell["policy"],
                cell["backend"],
                cell["accelerator_claim"],
            )
        )
    require(
        observed_matrix == expected_matrix and len(matrix) == len(expected_matrix) + 2,
        "measurement host/package matrix does not match the release proof lanes",
    )
    require(
        {
            cell_id
            for cell_id, cell in matrix.items()
            if cell["host_class"].startswith("premerge_candidate_")
        }
        == {
            "candidate_installed_linux_x64_cpu",
            "candidate_installed_macos_arm64_cpu",
        },
        "measurement matrix omitted the two candidate-installed premerge lanes",
    )


def _verify_calibration_matrix(matrix: dict, calibration_matrix: object) -> None:
    require(
        isinstance(calibration_matrix, dict)
        and set(calibration_matrix)
        == {
            "hosted_linux_x64_cpu",
            "protected_macos_arm64_metal",
        },
        "measurement calibration matrix must contain the Linux CPU and macOS Metal pre-publish lanes",
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


def _verify_measurement_matrices(protocol: dict) -> None:
    matrix = protocol.get("host_package_matrix")
    require(
        isinstance(matrix, dict), "measurement protocol omitted its host/package matrix"
    )
    _verify_host_package_matrix(matrix)
    _verify_calibration_matrix(matrix, protocol.get("calibration_matrix"))


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
        if metric == "retrieval_quality":
            require(
                count == MIN_RETRIEVAL_QUALITY_REPEATS
                and aggregation == "all_rows_pass_rate"
                and policy.get("external_contract")
                == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
                "retrieval quality sample policy changed",
            )
        elif metric in {
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
        if metric != "retrieval_quality":
            require(
                aggregation == expected_aggregation,
                f"measurement metric {metric} aggregation is not conservative",
            )
