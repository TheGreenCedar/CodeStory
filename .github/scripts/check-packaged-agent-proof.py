#!/usr/bin/env python3
"""Verify a packaged CodeStory executable and its per-user embedding server.

The runtime proof is deliberately multi-process. Two independent plugin hosts
must converge on one same-account local authority, one server, one native
worker, and one model load. Synthetic self-tests validate this verifier only;
they never stand in for package, installed-runtime, or hardware evidence.
"""

from __future__ import annotations

import argparse
from collections import Counter
import ctypes
import hashlib
import json
import math
import os
import queue
import re
import secrets
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zipfile
from pathlib import Path

from native_binary_contract import (
    NativeBinaryError,
    inspect_runtime_layout,
    runtime_artifact_role,
)


STATUS_URI = "codestory://status"
ENGINE_DIAGNOSTICS_URI = "codestory://diagnostics/retrieval-engine"
SERVER_PROOF_SCHEMA_VERSION = 1
QUALIFICATION_SCHEMA_VERSION = 1
PUBLICATION_FAULT_EVIDENCE_CONTRACT = "codestory-publication-lease-fault/v1"
FAULT_RECOVERY_CONSISTENCY_CONTRACT = "codestory-fault-recovery-search-consistency/v1"
RETRIEVAL_QUALITY_EVIDENCE_CONTRACT = "publishable-three-repeat-packet/v1"
MEMORY_EVIDENCE_CONTRACT = "codestory-five-process-memory/v1"
FAULT_RECOVERY_CONSISTENCY_CASES = 10
MIN_RETRIEVAL_QUALITY_REPEATS = 3
RELEASE_QUALITY_CORPUS_ID = "codestory-release-corpus-v1"
RELEASE_QUALITY_MODES = {"cold-cli": "cold_cli_packet"}
REQUIRED_HOLDOUT_TASK_FILES = {
    "axios-request-dispatch.task.json",
    "redis-server-event-loop.task.json",
    "ripgrep-search-pipeline.task.json",
}
EXTERNAL_QUALIFICATION_METRICS = {
    "retrieval_quality",
    "total_codestory_process_memory",
}
MEASUREMENT_PROTOCOL = (
    Path(__file__).resolve().parents[2]
    / "docs"
    / "testing"
    / "per-user-embedding-server-measurement-protocol.json"
)
SERVER_PROTOCOL = MEASUREMENT_PROTOCOL.with_name("per-user-embedding-server-protocol.json")
SERVER_CONSTANT_SET = MEASUREMENT_PROTOCOL.with_name("per-user-embedding-server-constant-set.json")
HOLDOUT_TASK_ROOT = (
    Path(__file__).resolve().parents[2] / "benchmarks" / "tasks" / "holdout-retrieval"
)
DEFAULT_QUERY = "RuntimeContext"
DEFAULT_QUESTION = "Explain how CodeStory prepares retrieval."
SOFTWARE_ADAPTERS = ("llvmpipe", "lavapipe", "warp", "software rasterizer", "swiftshader")
LEGACY_TOKENS = (
    "llama-server",
    "repair-worker",
    "port-allocations",
    "native-embedding",
    "retrieval-sidecars",
    "sidecars",
    "owner.pid",
    "server.pid",
)
LEGACY_HELP_TOKENS = ("llama-server", "sidecar", "repair", "consent", "download")
NATIVE_MANIFEST_FILE = "codestory-native-manifest.json"
NATIVE_ENGINE_MARKER_PREFIX = "codestory-native-engine-v1|"
NATIVE_ENGINE_MARKER_SUFFIX = "|end"
SERVER_PROOF_MARKER_PREFIX = "codestory-embedding-server-proof-v1|"
SERVER_PROOF_MARKER_SUFFIX = "|end"
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
SERVER_LIFECYCLES = {
    "absent",
    "listening",
    "waking",
    "resident",
    "sleeping",
    "draining",
    "unreachable",
    "exited",
}
RETRY_CLASSES = {
    "none",
    "after_delay",
    "after_capacity_change",
    "after_server_change",
    "after_owner_idle",
    "same_rpc_once",
    "terminal",
}
REQUIRED_SERVER_SCENARIOS = {
    "cold_race",
    "mixed_queue",
    "client_death",
    "server_crash",
    "worker_stall",
    "true_idle_respawn",
    "incompatible_owner",
    "frozen_owner",
}
LOWER_TIER_NONCLAIMS = {
    "answer_quality",
    "release_readiness",
    "cross_user_sharing",
    "cross_session_sharing",
    "bounded_bulk_starvation",
    "whole_server_takeover",
    "linux_gpu_execution",
}
TARGET_CONTRACTS = {
    "linux-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "x86_64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "linux-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "aarch64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "windows-x64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "x86_64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": "vulkan",
        "non_claim_reason": None,
    },
    "windows-arm64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "aarch64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "windows_arm64_accelerator_execution_is_not_protected",
    },
    "macos-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "x86_64-apple-darwin",
        "target_os": "macos",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "metal"],
        "linkage": "static",
        "backend_loading": "builtin",
        "expected_protected_backend": None,
        "non_claim_reason": "macos_x64_accelerator_execution_is_not_protected",
    },
    "macos-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "aarch64-apple-darwin",
        "target_os": "macos",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "metal"],
        "linkage": "static",
        "backend_loading": "builtin",
        "expected_protected_backend": "metal",
        "non_claim_reason": None,
    },
}


class ProofFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProofFailure(message)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_sha256(value: object) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def retained_mcp_transcript(transcript: list[dict]) -> dict:
    entries = []
    for item in transcript:
        request = item.get("request") if isinstance(item, dict) else None
        response = item.get("response") if isinstance(item, dict) else None
        require(
            isinstance(request, dict) and isinstance(response, dict),
            "MCP transcript entry is malformed",
        )
        entries.append(
            {
                "request_id_sha256": canonical_sha256(request.get("id")),
                "method": require_nonempty_string(
                    request.get("method"), "MCP transcript method"
                ),
                "response_status": "error" if "error" in response else "ok",
            }
        )
    return {
        "schema_version": 1,
        "entry_count": len(entries),
        "raw_transcript_sha256": canonical_sha256(transcript),
        "entries": entries,
    }


def retained_runtime_evidence(runtime: dict) -> dict:
    return json.loads(
        json.dumps(
            {key: value for key, value in runtime.items() if not key.startswith("_")}
        )
    )


def assert_retained_json_privacy(target: Path, forbidden_values: list[str]) -> None:
    forbidden = sorted(
        {value for value in forbidden_values if isinstance(value, str) and value}
    )
    paths = [target] if target.is_file() else sorted(target.rglob("*.json"))
    for path in paths:
        require(path.is_file() and not path.is_symlink(), f"retained JSON is unsafe: {path}")
        payload = path.read_bytes()
        for value in forbidden:
            require(
                value.encode("utf-8") not in payload,
                f"retained evidence {path.name} leaked private runtime material",
            )
        try:
            document = json.loads(payload)
        except json.JSONDecodeError as exc:
            raise ProofFailure(f"retained evidence {path.name} is not valid JSON: {exc}") from exc

        def scan(value: object, field_path: str = "$") -> None:
            if isinstance(value, dict):
                for key, child in value.items():
                    require(isinstance(key, str), f"retained evidence {path.name} has a non-string field")
                    normalized = key.lower()
                    require(
                        normalized
                        not in {
                            "directory",
                            "output_directory",
                            "project_path",
                            "project_root",
                            "repository_path",
                            "request_text",
                            "query_text",
                            "question_text",
                            "qualification_nonce",
                        },
                        f"retained evidence {path.name} leaked private field {field_path}.{key}",
                    )
                    scan(child, f"{field_path}.{key}")
            elif isinstance(value, list):
                for index, child in enumerate(value):
                    scan(child, f"{field_path}[{index}]")
            elif isinstance(value, str):
                require(
                    not Path(value).is_absolute(),
                    f"retained evidence {path.name} leaked an absolute path at {field_path}",
                )

        scan(document)


def require_sha256(value: object, field: str) -> str:
    require(
        isinstance(value, str)
        and HEX_SHA256.fullmatch(value) is not None
        and value != "0" * 64,
        f"{field} must be a lowercase SHA-256 digest",
    )
    return value


def require_nonempty_string(value: object, field: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{field} must be a non-empty string")
    return value


def require_nonnegative_int(value: object, field: str) -> int:
    require(
        isinstance(value, int) and not isinstance(value, bool) and value >= 0,
        f"{field} must be a non-negative integer",
    )
    return value


def require_positive_int(value: object, field: str) -> int:
    value = require_nonnegative_int(value, field)
    require(value > 0, f"{field} must be positive")
    return value


def require_exact_keys(value: dict, expected: set[str], field: str) -> None:
    actual = set(value)
    require(
        actual == expected,
        f"{field} fields differ from the contract; missing={sorted(expected - actual)}, "
        f"unknown={sorted(actual - expected)}",
    )


def load_measurement_protocol(path: Path) -> tuple[dict, str]:
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
            and all(isinstance(assertion, str) and assertion for assertion in contract["required"]),
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
        require(isinstance(contract, dict), f"measurement metric {metric} contract is malformed")
        require(
            contract.get("comparison")
            in {"equal", "greater_than_or_equal", "less_than_or_equal"},
            f"measurement metric {metric} has an unsupported comparison",
        )
        require_nonempty_string(contract.get("unit"), f"measurement metric {metric} unit")
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
    matrix = protocol.get("host_package_matrix")
    require(isinstance(matrix, dict), "measurement protocol omitted its host/package matrix")
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
        require(isinstance(cell, dict), f"measurement matrix cell {cell_id} is malformed")
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
        require_nonempty_string(cell["host_class"], f"measurement matrix cell {cell_id}.host_class")
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
    calibration_matrix = protocol.get("calibration_matrix")
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
    workloads = protocol.get("workloads")
    require(
        isinstance(workloads, dict) and set(workloads) == required_metrics,
        "measurement workloads do not match required metrics",
    )
    for metric, workload in workloads.items():
        require(isinstance(workload, dict), f"measurement workload {metric} is malformed")
        require_nonempty_string(workload.get("workload_id"), f"measurement workload {metric}.workload_id")
        require_nonempty_string(workload.get("owner_state"), f"measurement workload {metric}.owner_state")
        require_nonempty_string(workload.get("operation"), f"measurement workload {metric}.operation")
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
        require(isinstance(policy, dict), f"measurement sample policy {metric} is malformed")
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
                and policy.get("external_contract") == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
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
    constant_selection = protocol.get("constant_selection")
    require(
        isinstance(constant_selection, dict),
        "measurement protocol omitted production-constant selection",
    )
    require_exact_keys(
        constant_selection,
        {
            "selection_order",
            "raw_source_cells",
            "clean_run_requirements",
            "formulas",
            "post_result_formula_changes",
        },
        "measurement production-constant selection",
    )
    expected_constant_order = [
        "connect_timeout_ms",
        "spawn_convergence_timeout_ms",
        "hard_native_no_progress_ms",
        "watchdog_cadence_ms",
        "request_deadlines_ms",
        "capacity_retry_policy",
        "election_backoff_policy",
    ]
    require(
        constant_selection["selection_order"] == expected_constant_order,
        "production constants changed selection order",
    )
    source_cells = constant_selection["raw_source_cells"]
    require(
        isinstance(source_cells, dict)
        and set(source_cells)
        == {
            "existing_owner_connect_duration",
            "spawn_convergence_duration",
            "query_request_duration",
            "bulk_request_duration",
            "capacity_condition_duration",
            "successful_operation_duration",
        }
        and all(
            isinstance(cell, dict)
            and cell.get("artifact") == "measurements.raw.json"
            and isinstance(cell.get("operand"), str)
            and bool(cell["operand"])
            and (isinstance(cell.get("metric"), str) or isinstance(cell.get("metrics"), list))
            for cell in source_cells.values()
        ),
        "production constants do not name their exact raw source cells",
    )
    require(
        constant_selection["clean_run_requirements"]
        == {
            "minimum_runs_per_matrix_cell": 3,
            "matrix_coverage": "every_calibration_matrix_cell",
            "source_identity": "one_exact_candidate_commit_and_tree",
            "artifact_selection": "all_preregistered_clean_runs",
            "unplanned_suspend": False,
            "outlier_removal": "none",
        },
        "production-constant calibration run selection changed",
    )
    formulas = constant_selection["formulas"]
    require(
        isinstance(formulas, dict)
        and set(formulas) == set(expected_constant_order),
        "production-constant formulas are incomplete",
    )
    expected_formula_fragments = {
        "connect_timeout_ms": "maximum_raw_value_ms_across_all_selected_samples*1.50",
        "spawn_convergence_timeout_ms": "maximum_raw_value_ms_across_all_selected_samples*1.50",
        "hard_native_no_progress_ms": "maximum_complete_successful_operation_duration_ms_across_all_selected_samples*4.00",
        "watchdog_cadence_ms": "hard_native_no_progress_ms/20",
    }
    for field, fragment in expected_formula_fragments.items():
        require(
            isinstance(formulas[field], dict)
            and fragment in formulas[field].get("formula", ""),
            f"production-constant formula {field} changed",
        )
    require(
        formulas["request_deadlines_ms"].get("query_request_deadline_ms", {}).get(
            "formula"
        )
        == "max(1,ceiling(maximum_raw_value_ms_across_all_selected_samples*1.50))"
        and formulas["request_deadlines_ms"].get("bulk_request_deadline_ms", {}).get(
            "replay_success_budget_formula"
        )
        == "max(query_request_deadline_ms,ceiling(maximum_raw_value_ms_across_all_selected_samples*1.50))"
        and formulas["request_deadlines_ms"].get("bulk_request_deadline_ms", {}).get(
            "formula"
        )
        == "hard_native_no_progress_ms+watchdog_cadence_ms+spawn_convergence_timeout_ms+bulk_replay_success_budget_ms",
        "request-deadline selection formulas changed",
    )
    require(
        formulas["capacity_retry_policy"].get("retry_after_ms_formula")
        == "max(1,floor(minimum_raw_value_ms_across_all_selected_samples*0.50))"
        and formulas["capacity_retry_policy"].get("retry_class")
        == "after_capacity_change"
        and formulas["capacity_retry_policy"].get("retry_condition_source")
        == "named_condition_from_typed_capacity_response",
        "capacity retry selection formula or typed policy changed",
    )
    require(
        formulas["election_backoff_policy"].get("initial_backoff_ms_formula")
        == "max(1,ceiling(maximum_existing_owner_connect_duration_ms_across_all_selected_samples*0.50))"
        and formulas["election_backoff_policy"].get("maximum_backoff_ms_formula")
        == "max(initial_backoff_ms,ceiling(maximum_spawn_convergence_duration_ms_across_all_selected_samples*0.25))"
        and formulas["election_backoff_policy"].get("jitter")
        == "sha256(process_start_id||attempt) modulo inclusive [initial_backoff_ms,maximum_backoff_ms]",
        "election backoff selection formula changed",
    )
    require(
        constant_selection["post_result_formula_changes"] is False,
        "production constants allow post-result formula changes",
    )
    threshold_selection = protocol.get("threshold_selection")
    require(
        isinstance(threshold_selection, dict)
        and threshold_selection
        == {
            "minimum_clean_calibration_runs_per_matrix_cell": 3,
            "matrix_coverage": "every_calibration_matrix_cell",
            "source_identity": "one_exact_candidate_commit_and_tree",
            "producer_identity": (
                "trusted_packaged_platform_pr_workflow_run_and_exact_artifact"
            ),
            "artifact_selection": "all_preregistered_clean_runs",
            "less_than_or_equal": (
                "ceiling(maximum_cell_aggregate_across_all_runs*1.20)"
            ),
            "greater_than_or_equal": (
                "floor(minimum_cell_aggregate_across_all_runs*0.80)"
            ),
            "equal": "exact_observed_contract_value",
            "retrieval_quality": 1.0,
            "outlier_removal": "none",
            "post_result_threshold_changes": False,
        },
        "measurement threshold-selection formula is incomplete or mutable",
    )
    require(
        protocol.get("calibration_bundle_contract")
        == {
            "schema_version": 1,
            "required_for_frozen_qualification": True,
            "matrix_cells": "exactly_every_calibration_matrix_cell",
            "independent_clean_runs_per_matrix_cell": 3,
            "source_identity": "one_exact_candidate_commit_and_tree",
            "producer_identity": (
                "trusted_packaged_platform_pr_workflow_run_and_exact_artifact"
            ),
            "contract_identity": [
                "protocol_sha256",
                "measurement_protocol_sha256",
            ],
            "raw_artifact": (
                "embedded_product_and_five_process_measurements_with_canonical_sha256"
            ),
            "clock_witnesses": "awake_monotonic_plus_suspend_inclusive_per_sample",
            "successful_operation_operand": "successful_operation_duration_ns",
            "freeze_digest_inputs": [
                "selection_protocol",
                "source",
                "producer",
                "contracts",
                "run_artifact_sha256s",
                "calibration_required_values",
                "qualification_thresholds",
            ],
            "constant_set_comparison": (
                "exact_recomputed_values_thresholds_and_freeze_record"
            ),
            "qualification_boundary": (
                "installed_runtime_cells_are_post_freeze_qualification_only"
            ),
        },
        "measurement calibration-bundle contract is incomplete or mutable",
    )
    clock_policy = protocol.get("clock_policy")
    suspend = clock_policy.get("suspend_detection") if isinstance(clock_policy, dict) else None
    require(
        isinstance(clock_policy, dict)
        and clock_policy.get("cross_process_timestamp_subtraction") is False
        and clock_policy.get("server_idle_deadline_proof")
        == "server_event_elapsed_then_client_local_remaining_wait",
        "measurement clock policy permits cross-process idle-deadline arithmetic",
    )
    require(
        isinstance(suspend, dict)
        and suspend.get("maximum_inclusive_minus_awake_ns") == 50_000_000
        and suspend.get("platform_apis")
        == {
            "linux": "CLOCK_BOOTTIME",
            "macos": "mach_continuous_time",
            "windows": "QueryInterruptTimePrecise",
        },
        "measurement suspend-detection contract is incomplete",
    )
    return protocol, sha256(path)


def load_json_contract(path: Path, label: str) -> tuple[dict, str]:
    require(path.is_file(), f"{label} is missing: {path}")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(document, dict), f"{label} must be an object")
    require(document.get("schema_version") == 1, f"{label} schema is unsupported")
    return document, sha256(path)


def load_server_measurement_contract(measurement_protocol_path: Path) -> dict:
    measurement, measurement_sha256 = load_measurement_protocol(
        measurement_protocol_path
    )
    protocol_path = measurement_protocol_path.with_name(SERVER_PROTOCOL.name)
    constant_set_path = measurement_protocol_path.with_name(SERVER_CONSTANT_SET.name)
    protocol, protocol_sha256 = load_json_contract(
        protocol_path, "embedding server protocol"
    )
    constant_set, constant_set_sha256 = load_json_contract(
        constant_set_path,
        "embedding server constant set",
    )
    return {
        "measurement_protocol": measurement,
        "measurement_protocol_sha256": measurement_sha256,
        "protocol": protocol,
        "protocol_sha256": protocol_sha256,
        "constant_set": constant_set,
        "constant_set_sha256": constant_set_sha256,
    }


def load_holdout_task_contracts(root: Path = HOLDOUT_TASK_ROOT) -> tuple[dict[tuple[str, str], dict], str]:
    require(root.is_dir(), f"holdout retrieval task directory is missing: {root}")
    paths = sorted(root.glob("*.task.json"))
    require(
        {path.name for path in paths} == REQUIRED_HOLDOUT_TASK_FILES,
        "checked-in holdout retrieval task set changed without updating the release contract",
    )
    tasks: dict[tuple[str, str], dict] = {}
    corpus_records = []
    for path in paths:
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise ProofFailure(f"holdout task manifest is not valid JSON: {path}: {exc}") from exc
        require(isinstance(raw, dict), f"holdout task manifest must be an object: {path}")
        require(raw.get("version") == 1, f"holdout task manifest schema is unsupported: {path}")
        task_id = require_nonempty_string(raw.get("id"), f"holdout task {path.name}.id")
        require(
            path.name == f"{task_id}.task.json",
            f"holdout task id does not match its checked-in filename: {path}",
        )
        require(raw.get("suite") == "holdout-retrieval", f"holdout task {task_id} left the release suite")
        repo = raw.get("repo")
        require(isinstance(repo, dict), f"holdout task {task_id} omitted repository identity")
        repo_name = require_nonempty_string(repo.get("name"), f"holdout task {task_id} repo.name")
        require(
            isinstance(repo.get("url"), str)
            and re.fullmatch(
                r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?",
                repo["url"],
            )
            is not None,
            f"holdout task {task_id} repository URL is not trusted",
        )
        require(
            isinstance(repo.get("ref"), str)
            and re.fullmatch(r"[0-9a-f]{40}", repo["ref"]) is not None,
            f"holdout task {task_id} repository ref is not immutable",
        )
        key = (repo_name, task_id)
        require(key not in tasks, f"holdout task identity is duplicated: {repo_name}/{task_id}")
        expected_snapshot = {
            "id": task_id,
            "name": raw.get("name", task_id),
            "suite": "holdout-retrieval",
            "repo": repo_name,
            "repo_metadata": repo,
            "task_class": raw.get("task_class"),
            "prompt": raw.get("prompt"),
            "expected_files": raw.get("expected_files", []),
            "expected_verification_files": raw.get("expected_verification_files", []),
            "expected_symbols": raw.get("expected_symbols", []),
            "expected_symbol_probes": raw.get("expected_symbol_probes", []),
            "expected_claims": raw.get("expected_claims", []),
            "forbidden_claims": raw.get("forbidden_claims", []),
            "quality_thresholds": raw.get("quality_thresholds", {}),
        }
        tasks[key] = {
            "path": path,
            "manifest_sha256": sha256(path),
            "snapshot": expected_snapshot,
            "repo": repo,
        }
        corpus_records.append(
            {
                "path": path.relative_to(Path(__file__).resolve().parents[2]).as_posix(),
                "sha256": tasks[key]["manifest_sha256"],
            }
        )
    return tasks, canonical_sha256(corpus_records)


def verify_package_server_contracts(
    manifest: dict,
    measurement_protocol_path: Path,
    *,
    require_frozen: bool,
) -> dict:
    contract = load_server_measurement_contract(measurement_protocol_path)
    measurement = contract["measurement_protocol"]
    measurement_sha256 = contract["measurement_protocol_sha256"]
    protocol = contract["protocol"]
    protocol_sha256 = contract["protocol_sha256"]
    constant_set = contract["constant_set"]
    constant_set_sha256 = contract["constant_set_sha256"]
    server_proof = manifest.get("server_proof")
    require(isinstance(server_proof, dict), "package manifest omitted server_proof")
    expected = {
        "measurement_protocol_sha256": measurement_sha256,
        "protocol_sha256": protocol_sha256,
        "constant_set_sha256": constant_set_sha256,
    }
    for field, digest in expected.items():
        require(
            server_proof.get(field) == digest,
            f"package manifest {field} does not match the checked-in contract",
        )
    require(
        server_proof.get("constant_set_status") == constant_set.get("status"),
        "package manifest constant-set status does not match the checked-in contract",
    )
    require(
        set(protocol.get("lifecycle_states", [])) == SERVER_LIFECYCLES,
        "embedding server lifecycle states do not match the verifier",
    )
    required_metrics = set(measurement["required_metrics"])
    thresholds = constant_set.get("qualification_thresholds")
    require(
        isinstance(thresholds, dict) and set(thresholds) == required_metrics,
        "embedding server qualification thresholds do not match the measurement metrics",
    )
    if require_frozen:
        require(
            constant_set.get("status") == "frozen",
            "embedding server constants are not frozen; calibration cannot be treated as qualification",
        )
        freeze_record = constant_set.get("freeze_record")
        require(isinstance(freeze_record, dict), "frozen embedding server constants omit their freeze record")
        require_exact_keys(
            freeze_record,
            {
                "selection_source_commit",
                "selection_source_tree",
                "measurement_protocol_sha256",
                "protocol_sha256",
                "input_constant_set_sha256",
                "calibration_bundle_sha256",
                "calibration_freeze_digest",
                "run_artifact_sha256s",
                "selection_rule",
                "selected_at",
            },
            "constant-set freeze_record",
        )
        for field in (
            "selection_source_commit",
            "selection_source_tree",
            "measurement_protocol_sha256",
            "protocol_sha256",
            "input_constant_set_sha256",
            "calibration_bundle_sha256",
            "calibration_freeze_digest",
            "selection_rule",
            "selected_at",
        ):
            require_nonempty_string(
                freeze_record.get(field),
                f"constant-set freeze_record.{field}",
            )
        for field in ("selection_source_commit", "selection_source_tree"):
            require(
                re.fullmatch(r"[0-9a-f]{40}", freeze_record[field]) is not None,
                f"constant-set freeze_record.{field} must be a lowercase Git object id",
            )
        for field in (
            "measurement_protocol_sha256",
            "protocol_sha256",
            "input_constant_set_sha256",
            "calibration_bundle_sha256",
            "calibration_freeze_digest",
        ):
            require_sha256(freeze_record[field], f"constant-set freeze_record.{field}")
        run_digests = freeze_record["run_artifact_sha256s"]
        required_run_count = len(measurement["calibration_matrix"]) * 3
        require(
            isinstance(run_digests, list)
            and len(run_digests) == required_run_count
            and len(set(run_digests)) == required_run_count,
            "constant-set freeze record must bind three distinct runs for every calibration cell",
        )
        for index, digest in enumerate(run_digests):
            require_sha256(digest, f"constant-set freeze_record.run_artifact_sha256s[{index}]")
        unresolved = [
            field
            for section in ("calibration_required_values", "qualification_thresholds")
            for field, value in constant_set.get(section, {}).items()
            if value is None
        ]
        require(not unresolved, "frozen embedding server constants contain unresolved values: " + ", ".join(unresolved))
    return contract


def expected_archive_digest(checksum_file: Path, archive: Path) -> str:
    lines = checksum_file.read_text(encoding="utf-8").splitlines()
    records: dict[str, str] = {}
    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 2 and len(parts[0]) == 64:
            records[parts[-1].lstrip("*")] = parts[0].lower()
        elif len(parts) == 1 and len(parts[0]) == 64:
            records[archive.name] = parts[0].lower()
    require(archive.name in records, f"checksum file does not name {archive.name}")
    return records[archive.name]


def safe_target(root: Path, name: str) -> Path:
    target = (root / name).resolve()
    require(target.is_relative_to(root.resolve()), f"archive member escapes extraction root: {name}")
    return target


def unpack_archive(archive: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                safe_target(destination, member.filename)
            handle.extractall(destination)
        return
    if tarfile.is_tarfile(archive):
        with tarfile.open(archive) as handle:
            members = handle.getmembers()
            for member in members:
                safe_target(destination, member.name)
                require(not member.issym() and not member.islnk(), f"archive contains link: {member.name}")
            handle.extractall(destination, members=members)
        return
    raise ProofFailure(f"unsupported archive format: {archive}")


def find_cli(root: Path) -> Path:
    names = {"codestory-cli", "codestory-cli.exe"}
    matches = [path for path in root.rglob("*") if path.is_file() and path.name in names]
    require(len(matches) == 1, f"archive must contain exactly one native CodeStory executable; found {len(matches)}")
    cli = matches[0]
    cli.chmod(cli.stat().st_mode | stat.S_IXUSR)
    return cli


def binary_markers(path: Path, marker_prefix: str, marker_suffix: str) -> list[str]:
    prefix = marker_prefix.encode("ascii")
    suffix = marker_suffix.encode("ascii")
    markers: set[bytes] = set()
    overlap = b""
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            block = overlap + chunk
            offset = 0
            while True:
                start = block.find(prefix, offset)
                if start < 0:
                    break
                end = block.find(suffix, start)
                if end < 0:
                    break
                end += len(suffix)
                markers.add(block[start:end])
                offset = end
            overlap = block[-4096:]
    decoded = []
    for marker in sorted(markers):
        try:
            decoded.append(marker.decode("ascii"))
        except UnicodeDecodeError as exc:
            raise ProofFailure(f"packaged marker {marker_prefix!r} is not ASCII") from exc
    return decoded


def native_engine_markers(path: Path) -> list[str]:
    return binary_markers(path, NATIVE_ENGINE_MARKER_PREFIX, NATIVE_ENGINE_MARKER_SUFFIX)


def server_proof_markers(path: Path) -> list[str]:
    return binary_markers(path, SERVER_PROOF_MARKER_PREFIX, SERVER_PROOF_MARKER_SUFFIX)


def ordered_contract_digest(domain: str, values: list[str]) -> str:
    digest = hashlib.sha256()
    for value in [domain, *values]:
        encoded = value.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "little"))
        digest.update(encoded)
    return digest.hexdigest()


def embedding_contract_digest(model: dict, embedding: dict, tokenizer: dict) -> str:
    string_fields = [
        (model, "file_name", False),
        (model, "sha256", False),
        (embedding, "family", False),
        (embedding, "query_prefix", False),
        (embedding, "document_prefix", True),
        (embedding, "pooling", False),
        (embedding, "normalization", False),
        (embedding, "element_type", False),
        (tokenizer, "container", False),
        (tokenizer, "tokenizer_sha256", False),
        (tokenizer, "config_sha256", False),
    ]
    for owner, field, allow_empty in string_fields:
        value = owner.get(field)
        require(
            isinstance(value, str) and (allow_empty or bool(value)),
            f"native embedding contract field {field} is invalid",
        )
    for owner, field in (
        (model, "size_bytes"),
        (embedding, "dimension"),
        (embedding, "vector_schema_version"),
    ):
        require(
            type(owner.get(field)) is int and owner[field] > 0,
            f"native embedding contract field {field} is invalid",
        )
    return ordered_contract_digest(
        "codestory-native-embedding-contract-v1",
        [
            model["file_name"],
            str(model["size_bytes"]),
            model["sha256"],
            embedding["family"],
            str(embedding["dimension"]),
            embedding["query_prefix"],
            embedding["document_prefix"],
            embedding["pooling"],
            embedding["normalization"],
            embedding["element_type"],
            str(embedding["vector_schema_version"]),
            tokenizer["container"],
            tokenizer["tokenizer_sha256"],
            tokenizer["config_sha256"],
        ],
    )


def parse_native_build_identity(identity: str) -> dict[str, str]:
    parts = identity.split("|")
    require(parts[0] == "codestory-native-engine-v1", "native engine build schema is unsupported")
    require(parts[-1] == "end", "native engine build identity terminator is missing")
    fields: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed native engine build field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty native engine build field: {part!r}")
        require(key not in fields, f"duplicate native engine build field: {key}")
        fields[key] = value
    required = {
        "target",
        "os",
        "arch",
        "linkage",
        "backend_loading",
        "backends",
        "llama_cpp_crate",
        "llama_cpp_commit",
        "model_sha256",
        "embedding_contract_sha256",
        "model_embedded",
        "producer",
    }
    missing = sorted(required - fields.keys())
    require(not missing, "native engine build identity is missing fields: " + ", ".join(missing))
    return fields


def parse_server_proof_identity(identity: str) -> dict[str, object]:
    parts = identity.split("|")
    require(
        parts[0] == SERVER_PROOF_MARKER_PREFIX.removesuffix("|"),
        "embedding server proof schema is unsupported",
    )
    require(parts[-1] == "end", "embedding server proof identity terminator is missing")
    raw: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed embedding server proof field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty embedding server proof field: {part!r}")
        require(key not in raw, f"duplicate embedding server proof field: {key}")
        raw[key] = value
    required = {
        "bootstrap",
        "protocol_schema",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
        "clock_policy",
        "query_capacity",
        "bulk_capacity",
        "idle_timeout_ms",
    }
    missing = sorted(required - raw.keys())
    require(not missing, "embedding server proof identity is missing fields: " + ", ".join(missing))
    require(set(raw) == required, "embedding server proof identity contains unknown fields")
    for field in ("protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"):
        require_sha256(raw[field], f"embedding server proof {field}")
    require(raw["clock_policy"] == "awake_monotonic", "embedding server proof clock policy is unsupported")
    numeric: dict[str, int] = {}
    for field in ("bootstrap", "protocol_schema", "query_capacity", "bulk_capacity", "idle_timeout_ms"):
        try:
            numeric[field] = int(raw[field])
        except ValueError as exc:
            raise ProofFailure(f"embedding server proof {field} is not an integer") from exc
        require(numeric[field] > 0, f"embedding server proof {field} must be positive")
    require(numeric["bootstrap"] == 1, "embedding server bootstrap version is unsupported")
    require(numeric["protocol_schema"] == 1, "embedding server protocol schema is unsupported")
    require(numeric["query_capacity"] == 64, "embedding server query capacity is not the accepted value")
    require(numeric["bulk_capacity"] == 64, "embedding server bulk capacity is not the accepted value")
    require(numeric["idle_timeout_ms"] == 60_000, "embedding server idle timeout is not the accepted value")
    return {
        "schema_version": SERVER_PROOF_SCHEMA_VERSION,
        "bootstrap_version": numeric["bootstrap"],
        "protocol_schema_version": numeric["protocol_schema"],
        "protocol_sha256": raw["protocol_sha256"],
        "constant_set_sha256": raw["constant_set_sha256"],
        "measurement_protocol_sha256": raw["measurement_protocol_sha256"],
        "clock_policy": raw["clock_policy"],
        "query_capacity": numeric["query_capacity"],
        "bulk_capacity": numeric["bulk_capacity"],
        "idle_timeout_ms": numeric["idle_timeout_ms"],
        "lower_tier_nonclaims": sorted(LOWER_TIER_NONCLAIMS),
    }


def load_native_manifest(root: Path, cli: Path, expected_version: str) -> dict:
    matches = [path for path in root.rglob(NATIVE_MANIFEST_FILE) if path.is_file()]
    require(
        len(matches) == 1,
        f"archive must contain exactly one native engine manifest; found {len(matches)}",
    )
    try:
        manifest = json.loads(matches[0].read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"native engine manifest is not valid JSON: {exc}") from exc
    require(isinstance(manifest, dict), "native engine manifest is not an object")
    require(manifest.get("schema_version") == 3, "native engine manifest schema is unsupported")
    require(
        manifest.get("release_version") == expected_version,
        "native engine manifest version does not match expected release",
    )
    asset_target = manifest.get("asset_target")
    target_contract = TARGET_CONTRACTS.get(asset_target)
    require(target_contract is not None, f"native manifest has unsupported asset target: {asset_target}")

    binary = manifest.get("binary")
    source = manifest.get("source")
    engine = manifest.get("engine")
    model = manifest.get("model")
    embedding = manifest.get("embedding")
    tokenizer = manifest.get("tokenizer_config")
    accelerator = manifest.get("accelerator")
    server_proof = manifest.get("server_proof")
    runtime_artifacts = manifest.get("runtime_artifacts")
    require(isinstance(binary, dict), "native engine manifest has no binary descriptor")
    require(isinstance(source, dict), "native engine manifest has no source descriptor")
    for field in ("commit", "tree"):
        value = source.get(field)
        require(
            isinstance(value, str)
            and len(value) == 40
            and all(char in "0123456789abcdef" for char in value),
            f"native engine manifest source {field} is invalid",
        )
    require(source.get("tracked_dirty") is False, "native engine manifest was built from tracked changes")
    require(isinstance(engine, dict), "native engine manifest has no engine descriptor")
    require(isinstance(model, dict), "native engine manifest has no model descriptor")
    require(isinstance(embedding, dict), "native engine manifest has no embedding descriptor")
    require(isinstance(tokenizer, dict), "native engine manifest has no tokenizer descriptor")
    require(isinstance(accelerator, dict), "native engine manifest has no accelerator descriptor")
    require(isinstance(server_proof, dict), "native engine manifest has no server proof descriptor")
    require(isinstance(runtime_artifacts, list), "native engine manifest has no runtime artifact set")
    require(
        cli.name == target_contract["binary_name"],
        "packaged executable name does not match asset target",
    )
    require(binary.get("name") == cli.name, "native engine manifest names a different binary")
    cli_sha256 = sha256(cli)
    require(binary.get("sha256") == cli_sha256, "packaged binary digest does not match native manifest")
    artifact_paths: list[Path] = []
    for descriptor in runtime_artifacts:
        require(isinstance(descriptor, dict), "native runtime artifact descriptor is invalid")
        name = descriptor.get("name")
        require(
            isinstance(name, str) and name == Path(name).name,
            "native runtime artifact name is not a basename",
        )
        path = cli.parent / name
        require(path.is_file(), f"native runtime artifact is missing: {name}")
        artifact_paths.append(path)
    discovered = sorted(
        [
            path.name
            for path in cli.parent.iterdir()
            if path.is_file()
            and runtime_artifact_role(path.name, target_contract["target_os"]) is not None
        ],
        key=str.lower,
    )
    require(
        discovered == sorted([path.name for path in artifact_paths], key=str.lower),
        "archive native runtime artifacts do not match the manifest",
    )
    try:
        binary_identity, inspected_artifacts = inspect_runtime_layout(
            cli,
            artifact_paths,
            target_os=target_contract["target_os"],
            expected_format=target_contract["binary_format"],
            expected_arch=target_contract["target_arch"],
            linkage=target_contract["linkage"],
            backend_loading=target_contract["backend_loading"],
        )
    except (OSError, NativeBinaryError) as exc:
        raise ProofFailure(f"packaged native runtime layout is invalid: {exc}") from exc
    inspected_artifacts = [
        {**descriptor, "sha256": sha256(cli.parent / str(descriptor["name"]))}
        for descriptor in inspected_artifacts
    ]
    require(runtime_artifacts == inspected_artifacts, "native runtime artifact evidence is stale")
    require(binary == {
        "name": cli.name,
        "sha256": cli_sha256,
        "format": binary_identity["format"],
        "arch": binary_identity["arch"],
        "needed": binary_identity["needed"],
    }, "native manifest binary descriptor does not match the executable")
    require(
        binary_identity["format"] == target_contract["binary_format"],
        "packaged executable format does not match asset target",
    )
    require(
        binary_identity["arch"] == target_contract["target_arch"],
        "packaged executable architecture does not match asset target",
    )
    require(engine.get("build_contract_schema_version") == 2, "native engine build contract is unsupported")
    build_identity = engine.get("build_identity")
    require(
        isinstance(build_identity, str)
        and build_identity.startswith(NATIVE_ENGINE_MARKER_PREFIX)
        and build_identity.endswith("|end"),
        "native engine build identity is malformed",
    )
    build_fields = parse_native_build_identity(build_identity)
    binary_markers = native_engine_markers(cli)
    require(
        binary_markers == [build_identity],
        "packaged executable native engine marker does not match manifest",
    )
    server_markers = server_proof_markers(cli)
    require(
        len(server_markers) == 1,
        "packaged executable must contain exactly one embedding server proof marker",
    )
    marker_server_proof = parse_server_proof_identity(server_markers[0])
    require(
        set(server_proof) == {*marker_server_proof, "constant_set_status"},
        "native manifest server proof fields do not match the binary marker schema",
    )
    for field, value in marker_server_proof.items():
        require(
            server_proof.get(field) == value,
            f"packaged executable embedding server proof field {field} does not match manifest",
        )
    require(
        server_proof.get("constant_set_status") in {"unfrozen", "frozen"},
        "native manifest server proof constant-set status is invalid",
    )
    require(engine.get("linkage") == target_contract["linkage"], "packaged native engine linkage is wrong")
    require(build_fields["linkage"] == engine["linkage"], "native build linkage contradicts manifest")
    require(
        engine.get("backend_loading") == target_contract["backend_loading"],
        "packaged native backend loading mode is wrong",
    )
    require(
        build_fields["backend_loading"] == engine["backend_loading"],
        "native backend loading mode contradicts manifest",
    )
    compiled_backends = engine.get("compiled_backends")
    require(
        isinstance(compiled_backends, list)
        and compiled_backends
        and all(isinstance(item, str) and item for item in compiled_backends),
        "native manifest has no compiled backend set",
    )
    require(compiled_backends[0] == "cpu", "native manifest does not make CPU capability explicit")
    require(
        build_fields["backends"].split(",") == compiled_backends,
        "native build backend set contradicts manifest",
    )
    for manifest_field, build_field in (
        ("target_triple", "target"),
        ("target_os", "os"),
        ("target_arch", "arch"),
        ("llama_cpp_crate_version", "llama_cpp_crate"),
        ("llama_cpp_source_commit", "llama_cpp_commit"),
    ):
        require(
            engine.get(manifest_field) == build_fields[build_field],
            f"native build {build_field} contradicts manifest",
        )
    for manifest_field, expected in (
        ("target_triple", target_contract["target_triple"]),
        ("target_os", target_contract["target_os"]),
        ("target_arch", target_contract["target_arch"]),
    ):
        require(
            engine.get(manifest_field) == expected,
            f"native manifest {manifest_field} does not match asset target",
        )
    require(
        compiled_backends == target_contract["compiled_backends"],
        "native manifest backend set does not match asset target",
    )
    model_digest = model.get("sha256")
    require(
        isinstance(model_digest, str)
        and len(model_digest) == 64
        and all(char in "0123456789abcdefABCDEF" for char in model_digest),
        "native manifest lacks an exact model digest",
    )
    require(model.get("embedded") is True, "native manifest does not prove an embedded model")
    require(build_fields["model_embedded"] == "true", "native build marker says the model is absent")
    require(build_fields["model_sha256"] == model_digest, "native build model digest contradicts manifest")
    contract_sha256 = embedding_contract_digest(model, embedding, tokenizer)
    require(
        build_fields["embedding_contract_sha256"] == contract_sha256,
        "native build embedding contract contradicts manifest",
    )
    require(
        engine.get("embedding_contract_sha256") == contract_sha256,
        "native engine embedding contract digest contradicts manifest",
    )
    producer = model.get("producer")
    require(isinstance(producer, dict), "native manifest lacks model producer identity")
    require(
        build_fields["producer"] == f"{producer.get('name')}@{producer.get('version')}",
        "native build producer contradicts manifest",
    )
    require(
        producer.get("version") == expected_version,
        "native model producer version does not match expected release",
    )
    require(
        accelerator.get("cpu_fallback") == "explicit_only",
        "native manifest permits implicit CPU fallback",
    )
    require(
        accelerator.get("package_claim") == "compiled_capability_only",
        "native manifest overstates package-time accelerator proof",
    )
    require(
        accelerator.get("runtime_execution") == "not_proven_by_package",
        "native manifest overstates package-time execution proof",
    )
    expected_backend = accelerator.get("expected_protected_backend")
    require(
        expected_backend == target_contract["expected_protected_backend"],
        "native manifest protected backend does not match asset target",
    )
    require(
        accelerator.get("non_claim_reason") == target_contract["non_claim_reason"],
        "native manifest accelerator non-claim does not match asset target",
    )
    return manifest


def normalized_backend(value: object) -> str:
    backend = str(value or "").strip().lower()
    if backend == "mtl":
        return "metal"
    if backend.startswith("vulkan"):
        return "vulkan"
    return backend


def verify_runtime_against_manifest(
    manifest: dict,
    runtime: dict,
    expected_policy: str | None,
) -> dict:
    identities = [
        runtime.get("identity"),
        runtime.get("second_host_identity"),
        runtime.get("rejoin_identity"),
    ]
    require(all(isinstance(identity, dict) for identity in identities), "runtime proof omitted engine identity")
    engine = manifest["engine"]
    model = manifest["model"]
    accelerator = manifest["accelerator"]
    compiled_backends = engine["compiled_backends"]
    observed_backend = ""
    for label, identity in zip(("first plugin host", "second plugin host", "rejoined plugin host"), identities):
        require(
            identity.get("embedding_ggml_build_identity") == engine["build_identity"],
            f"{label} loaded a different native engine build than the package manifest",
        )
        require(
            identity.get("embedding_model_sha256") == model["sha256"],
            f"{label} loaded a different embedding model than the package manifest",
        )
        current_backend = normalized_backend(identity.get("embedding_backend"))
        require(
            current_backend in compiled_backends,
            f"{label} selected backend {current_backend!r} outside the compiled package contract",
        )
        if not observed_backend:
            observed_backend = current_backend
        require(current_backend == observed_backend, "native backend changed across process restart")

    policy = str(identities[0].get("embedding_policy") or "")
    require(policy == expected_policy, "runtime policy does not match the requested proof lane")
    if policy == "accelerated":
        expected_backend = accelerator.get("expected_protected_backend")
        require(
            isinstance(expected_backend, str) and bool(expected_backend),
            "this package target has no protected accelerator execution claim",
        )
        require(
            observed_backend == expected_backend,
            "runtime accelerator backend does not match the protected package contract",
        )
        execution = "proven_by_live_runtime"
        non_claim_reason = None
    else:
        require(policy == "cpu_explicit", "runtime used neither protected acceleration nor explicit CPU")
        require(observed_backend == "cpu", "explicit CPU proof selected a non-CPU backend")
        execution = "explicit_cpu_execution"
        non_claim_reason = (
            accelerator.get("non_claim_reason")
            or "explicit_cpu_execution_does_not_prove_acceleration"
        )

    return {
        "build_identity": engine["build_identity"],
        "model_sha256": model["sha256"],
        "policy": policy,
        "backend": observed_backend,
        "execution": execution,
        "answer_quality_claim": False,
        "non_claim_reason": non_claim_reason,
    }


def run(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> dict:
    started = time.perf_counter()
    completed = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout)
    result = {
        "command": command,
        "exit_code": completed.returncode,
        "wall_ms": round((time.perf_counter() - started) * 1000, 3),
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    if completed.returncode != 0:
        stdout_tail = completed.stdout[-2000:].strip()
        stderr_tail = completed.stderr[-2000:].strip()
        details = "\n".join(
            part
            for part in (
                f"stdout:\n{stdout_tail}" if stdout_tail else "",
                f"stderr:\n{stderr_tail}" if stderr_tail else "",
            )
            if part
        )
        detail_suffix = f"\n{details}" if details else ""
        raise ProofFailure(
            f"command failed ({completed.returncode}): {' '.join(command)}"
            f"{detail_suffix}"
        )
    return result


def json_command(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> tuple[dict, dict]:
    result = run(command, env=env, cwd=cwd, timeout=timeout)
    try:
        payload = json.loads(result["stdout"])
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"command did not emit JSON: {' '.join(command)}: {exc}") from exc
    require(isinstance(payload, dict), f"command emitted non-object JSON: {' '.join(command)}")
    return result, payload


def find_value(value: object, key: str) -> object | None:
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for child in value.values():
            found = find_value(child, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_value(child, key)
            if found is not None:
                return found
    return None


def engine_identity(
    status: dict,
    expected_policy: str | None,
    expected_backend: str | None,
    *,
    expected_load_count: int = 1,
    expected_load_generation: int = 1,
    expected_residency: str = "resident",
    expected_load_error: bool = False,
) -> dict:
    fields = {
        key: find_value(status, key)
        for key in (
            "embedding_model_sha256",
            "embedding_ggml_build_identity",
            "embedding_backend",
            "embedding_adapter",
            "embedding_policy",
            "embedding_engine_instance_id",
            "embedding_engine_residency",
            "embedding_engine_load_generation",
            "embedding_engine_load_error",
            "embedding_model_load_count",
            "embedding_smoke_ms",
            "embedding_initialization_ms",
            "embedding_materialized_path",
            "embedding_materialized_reused",
            "embedding_accelerator_execution_verified",
            "embedding_execution_devices",
            "embedding_execution_backends",
            "embedding_execution_observation_source",
            "embedding_encode_count",
            "embedding_execution_node_count",
            "embedding_resident_accelerator_tensor_count",
            "embedding_resident_accelerator_tensor_bytes",
            "embedding_model_layer_count",
            "embedding_offloaded_layer_count",
        )
    }
    digest = str(fields["embedding_model_sha256"] or "")
    require(len(digest) == 64 and all(char in "0123456789abcdefABCDEF" for char in digest), "status lacks an exact model digest")
    require(bool(fields["embedding_ggml_build_identity"]), "status lacks the linked ggml build identity")
    require(bool(fields["embedding_backend"]), "status lacks the selected embedding backend")
    adapter = str(fields["embedding_adapter"] or "")
    require(adapter, "status lacks the physical adapter identity")
    require(not any(token in adapter.lower() for token in SOFTWARE_ADAPTERS), f"software adapter is not allowed: {adapter}")
    require(fields["embedding_policy"] in {"accelerated", "cpu_explicit"}, "status lacks an explicit embedding policy")
    require(bool(fields["embedding_engine_instance_id"]), "status lacks the process engine identity")
    require(fields["embedding_engine_residency"] == expected_residency, f"engine residency is {fields['embedding_engine_residency']!r}, expected {expected_residency!r}")
    require(fields["embedding_model_load_count"] == expected_load_count, f"engine load count is {fields['embedding_model_load_count']!r}, expected {expected_load_count}")
    require(fields["embedding_engine_load_generation"] == expected_load_generation, f"engine load generation is {fields['embedding_engine_load_generation']!r}, expected {expected_load_generation}")
    if expected_load_error:
        require(bool(fields["embedding_engine_load_error"]), "failed reload did not retain its load error")
    else:
        require(fields["embedding_engine_load_error"] is None, f"engine retained an unexpected load error: {fields['embedding_engine_load_error']}")
    require(isinstance(fields["embedding_smoke_ms"], (int, float)) and fields["embedding_smoke_ms"] >= 0, "status lacks the timed live embedding smoke")
    require(isinstance(fields["embedding_initialization_ms"], (int, float)) and fields["embedding_initialization_ms"] >= 0, "status lacks initialization timing")
    if expected_policy:
        require(fields["embedding_policy"] == expected_policy, f"embedding policy is {fields['embedding_policy']!r}, expected {expected_policy!r}")
    if expected_backend:
        observed = str(fields["embedding_backend"] or "").lower()
        expected = expected_backend.lower()
        matches = expected in observed or (expected == "metal" and observed == "mtl")
        require(matches, f"embedding backend is {fields['embedding_backend']!r}, expected {expected_backend!r}")
    if fields["embedding_policy"] == "accelerated":
        require(fields["embedding_accelerator_execution_verified"] is True, "accelerated policy lacks live accelerator execution proof")
        require(
            fields["embedding_execution_observation_source"] == "ggml_eval_callback",
            "accelerator execution source is unknown or inferred",
        )
        require(
            isinstance(fields["embedding_execution_devices"], list)
            and bool(fields["embedding_execution_devices"]),
            "status lacks an observed execution device",
        )
        require(
            isinstance(fields["embedding_execution_backends"], list)
            and bool(fields["embedding_execution_backends"]),
            "status lacks an observed execution backend",
        )
        require(
            isinstance(fields["embedding_encode_count"], int)
            and fields["embedding_encode_count"] > 0,
            "status lacks an advancing successful encode counter",
        )
        require(
            isinstance(fields["embedding_execution_node_count"], int)
            and fields["embedding_execution_node_count"] > 0,
            "status lacks backend-observed execution nodes",
        )
        require(
            isinstance(fields["embedding_resident_accelerator_tensor_count"], int)
            and fields["embedding_resident_accelerator_tensor_count"] > 0,
            "status lacks backend-observed resident accelerator tensors",
        )
        require(
            isinstance(fields["embedding_resident_accelerator_tensor_bytes"], int)
            and fields["embedding_resident_accelerator_tensor_bytes"] > 0,
            "status lacks backend-observed resident accelerator tensor bytes",
        )
        model_layers = fields["embedding_model_layer_count"]
        offloaded_layers = fields["embedding_offloaded_layer_count"]
        require(isinstance(model_layers, int) and model_layers > 0, "status lacks model layer count")
        require(offloaded_layers == model_layers, "not every model layer was offloaded")
    return fields


def server_snapshot(status: dict, manifest: dict, *, require_resident: bool) -> dict:
    snapshot = find_value(status, "embedding_server")
    require(isinstance(snapshot, dict), "diagnostics omitted the embedding_server snapshot")
    require(
        snapshot.get("schema_version") == SERVER_PROOF_SCHEMA_VERSION,
        "embedding_server snapshot schema is unsupported",
    )
    event_sequence = require_nonnegative_int(
        snapshot.get("event_sequence"),
        "embedding_server.event_sequence",
    )
    lifecycle = snapshot.get("lifecycle")
    require(lifecycle in SERVER_LIFECYCLES, "embedding_server lifecycle is invalid")

    clock = snapshot.get("clock")
    require(isinstance(clock, dict), "embedding_server snapshot omitted clock identity")
    require(clock.get("domain") == "awake_monotonic", "embedding_server clock is not awake-monotonic")
    require_nonempty_string(clock.get("api"), "embedding_server.clock.api")
    require_nonempty_string(clock.get("boot_id"), "embedding_server.clock.boot_id")
    require_positive_int(clock.get("resolution_ns"), "embedding_server.clock.resolution_ns")

    protocol = snapshot.get("protocol")
    require(isinstance(protocol, dict), "embedding_server snapshot omitted protocol identity")
    require(protocol.get("bootstrap_version") == 1, "embedding_server bootstrap version is unsupported")
    require(protocol.get("schema_version") == 1, "embedding_server protocol version is unsupported")
    for field in ("protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"):
        require_sha256(protocol.get(field), f"embedding_server.protocol.{field}")

    server_proof = manifest.get("server_proof")
    require(isinstance(server_proof, dict), "package manifest omitted server_proof")
    for field in (
        "bootstrap_version",
        "protocol_schema_version",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    ):
        runtime_field = "schema_version" if field == "protocol_schema_version" else field
        require(
            protocol.get(runtime_field) == server_proof.get(field),
            f"runtime embedding server {runtime_field} does not match the package manifest",
        )

    authority = snapshot.get("authority")
    require(isinstance(authority, dict), "embedding_server snapshot omitted authority identity")
    for field in ("endpoint_namespace_id", "lifetime_authority_id", "listener_id"):
        require_nonempty_string(authority.get(field), f"embedding_server.authority.{field}")
    require(authority.get("peer_verified") is True, "embedding_server peer identity is not verified")

    process = snapshot.get("process")
    require(isinstance(process, dict), "embedding_server snapshot omitted process identity")
    for field in ("server_instance_id", "process_start_id", "executable_version"):
        require_nonempty_string(process.get(field), f"embedding_server.process.{field}")
    require_positive_int(process.get("pid"), "embedding_server.process.pid")
    require_sha256(process.get("executable_sha256"), "embedding_server.process.executable_sha256")
    require(
        process["executable_sha256"] == manifest["binary"]["sha256"],
        "embedding server process executable does not match the package manifest",
    )
    require(
        process["executable_version"] == manifest["release_version"],
        "embedding server process version does not match the package manifest",
    )

    scheduler = snapshot.get("scheduler")
    require(isinstance(scheduler, dict), "embedding_server snapshot omitted scheduler state")
    require(
        scheduler.get("query_capacity") == server_proof.get("query_capacity") == 64,
        "embedding_server query capacity is not the manifest-bound accepted value",
    )
    require(
        scheduler.get("bulk_capacity") == server_proof.get("bulk_capacity") == 64,
        "embedding_server bulk capacity is not the manifest-bound accepted value",
    )
    for field in (
        "query_depth",
        "bulk_depth",
        "connection_count",
        "active_request_count",
        "lease_count",
    ):
        require_nonnegative_int(scheduler.get(field), f"embedding_server.scheduler.{field}")
    require(scheduler["query_depth"] <= 64, "embedding_server query depth exceeds capacity")
    require(scheduler["bulk_depth"] <= 64, "embedding_server bulk depth exceeds capacity")
    active_request = scheduler.get("active_request")
    if active_request is not None:
        require(isinstance(active_request, dict), "embedding_server active request is malformed")
        for field in ("request_id", "scope_id", "class", "phase"):
            require_nonempty_string(
                active_request.get(field),
                f"embedding_server.scheduler.active_request.{field}",
            )
        require(active_request["class"] in {"query", "bulk"}, "active request class is invalid")
        require_nonnegative_int(
            active_request.get("elapsed_ms"),
            "embedding_server.scheduler.active_request.elapsed_ms",
        )

    engine = snapshot.get("engine")
    if require_resident:
        require(isinstance(engine, dict), "resident embedding_server snapshot omitted engine identity")
    if engine is not None:
        require(isinstance(engine, dict), "embedding_server engine identity is malformed")
        for field in ("engine_owner_id", "native_worker_id"):
            require_nonempty_string(engine.get(field), f"embedding_server.engine.{field}")
        require_positive_int(engine.get("load_generation"), "embedding_server.engine.load_generation")
        require_positive_int(engine.get("model_load_count"), "embedding_server.engine.model_load_count")
        require_nonnegative_int(
            engine.get("successful_encode_count"),
            "embedding_server.engine.successful_encode_count",
        )

    failure = snapshot.get("failure")
    if failure is not None:
        require(isinstance(failure, dict), "embedding_server failure state is malformed")
        require_nonempty_string(failure.get("code"), "embedding_server.failure.code")
        require(
            failure.get("retry_class") in RETRY_CLASSES,
            "embedding_server failure retry class is invalid",
        )
        require_nonnegative_int(
            failure.get("retry_after_ms"),
            "embedding_server.failure.retry_after_ms",
        )
        require_nonempty_string(
            failure.get("retry_condition"),
            "embedding_server.failure.retry_condition",
        )

    private_tokens = (
        str(snapshot).lower()
        if not isinstance(snapshot, (str, bytes))
        else str(snapshot).lower()
    )
    for forbidden in ("project_path", "project_root", "repository_path", "request_text"):
        require(forbidden not in private_tokens, f"embedding_server diagnostics leaked {forbidden}")
    return {
        "schema_version": snapshot["schema_version"],
        "event_sequence": event_sequence,
        "lifecycle": lifecycle,
        "clock": clock,
        "protocol": protocol,
        "authority": authority,
        "process": process,
        "scheduler": scheduler,
        "engine": engine,
        "failure": failure,
    }


def shared_server_identity(first: dict, second: dict) -> dict:
    for group, fields in (
        ("authority", ("endpoint_namespace_id", "lifetime_authority_id", "listener_id")),
        ("process", ("server_instance_id", "pid", "process_start_id", "executable_sha256")),
        ("engine", ("engine_owner_id", "native_worker_id", "load_generation", "model_load_count")),
    ):
        left = first.get(group)
        right = second.get(group)
        require(isinstance(left, dict) and isinstance(right, dict), f"shared proof omitted {group}")
        for field in fields:
            require(
                left.get(field) == right.get(field),
                f"independent plugin hosts observed different {group}.{field}",
            )
    require(
        first["engine"]["model_load_count"] == 1,
        "cold two-host race produced more than one model load",
    )
    return {
        "endpoint_namespace_id": first["authority"]["endpoint_namespace_id"],
        "lifetime_authority_id": first["authority"]["lifetime_authority_id"],
        "listener_id": first["authority"]["listener_id"],
        "server_instance_id": first["process"]["server_instance_id"],
        "server_process_start_id": first["process"]["process_start_id"],
        "engine_owner_id": first["engine"]["engine_owner_id"],
        "native_worker_id": first["engine"]["native_worker_id"],
        "load_generation": first["engine"]["load_generation"],
        "model_load_count": first["engine"]["model_load_count"],
    }


def verify_retained_qualification(
    evidence: dict,
    *,
    manifest: dict,
    archive_sha256: str,
    shared_identity: dict,
    measurement_contract: dict,
    required_tier: str,
    required_matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    expected_accelerator_claim: str,
    installed_plugin: dict | None = None,
    managed_runtime: dict | None = None,
) -> dict:
    require(
        evidence.get("schema_version") == QUALIFICATION_SCHEMA_VERSION,
        "retained qualification schema is unsupported",
    )
    require(evidence.get("status") == "pass", "retained qualification is not a passing result")
    tier = evidence.get("tier")
    require(
        tier in {"hosted_package", "protected_hardware", "installed_runtime"},
        "retained qualification tier is invalid",
    )
    require(
        tier == required_tier,
        f"retained {tier} evidence cannot support exact requested tier {required_tier}",
    )
    matrix_cell = selected_qualification_matrix_cell(
        measurement_contract["measurement_protocol"],
        cell_id=required_matrix_cell_id,
        target=manifest["asset_target"],
        proof_tier=required_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    require(
        matrix_cell["accelerator_claim"] == expected_accelerator_claim,
        "requested accelerator claim does not match the selected qualification matrix cell",
    )
    retained_plugin = evidence.get("installed_plugin")
    retained_runtime = evidence.get("managed_runtime")
    if tier == "installed_runtime":
        require(isinstance(retained_plugin, dict), "installed evidence omitted plugin provenance")
        require(isinstance(retained_runtime, dict), "installed evidence omitted managed runtime provenance")
        installation_source = retained_plugin.get("installation_source")
        require(
            installation_source in {"marketplace", "candidate_archive"}
            and retained_plugin.get("plugin_id") == "codestory"
            and retained_plugin.get("plugin_version") == manifest["release_version"],
            "installed evidence has invalid plugin provenance",
        )
        if installation_source == "marketplace":
            require(
                retained_plugin.get("marketplace_repository")
                == "TheGreenCedar/AgentPluginMarketplace"
                and retained_runtime.get("build_source") == "github_release"
                and retained_runtime.get("repo_ref")
                == f"v{manifest['release_version']}",
                "installed evidence has invalid marketplace/release provenance",
            )
            require(
                isinstance(retained_plugin.get("marketplace_commit"), str)
                and re.fullmatch(
                    r"[0-9a-f]{40}",
                    retained_plugin["marketplace_commit"],
                )
                is not None,
                "installed evidence marketplace commit is invalid",
            )
        else:
            producer = retained_plugin.get("producer")
            require(
                retained_plugin.get("candidate_archive_sha256")
                == archive_sha256
                and retained_plugin.get("candidate_asset_target")
                == manifest["asset_target"]
                and retained_plugin.get("plugin_source_tree")
                == manifest["source"]["tree"]
                and retained_runtime.get("build_source")
                == "candidate_archive"
                and retained_runtime.get("repo_ref")
                == manifest["source"]["commit"],
                "installed evidence has invalid staged-candidate provenance",
            )
            require(
                isinstance(producer, dict)
                and producer.get("repository") == "TheGreenCedar/CodeStory"
                and producer.get("workflow_path")
                == ".github/workflows/packaged-platform-pr.yml"
                and isinstance(producer.get("run_id"), str)
                and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
                and isinstance(producer.get("run_attempt"), str)
                and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"])
                is not None,
                "installed evidence has unauthenticated candidate producer identity",
            )
        require_sha256(
            retained_plugin.get("plugin_package_sha256"),
            "installed evidence plugin_package_sha256",
        )
        require(
            retained_plugin.get("plugin_source_commit") == manifest["source"]["commit"],
            "installed evidence does not bind the marketplace plugin to the packaged source commit",
        )
        require(
            retained_runtime.get("cli_source") == "managed"
            and retained_runtime.get("plugin_version") == manifest["release_version"]
            and retained_runtime.get("managed_binary_sha256") == manifest["binary"]["sha256"]
            and retained_runtime.get("archive_sha256") == archive_sha256,
            "installed evidence does not bind the exact managed runtime",
        )
        if installed_plugin is not None:
            require(retained_plugin == installed_plugin, "retained installed plugin provenance is stale")
        if managed_runtime is not None:
            require(retained_runtime == managed_runtime, "retained managed runtime provenance is stale")
    else:
        require(
            retained_plugin is None and retained_runtime is None,
            "lower-tier evidence must not claim installed plugin provenance",
        )

    source = evidence.get("source")
    require(isinstance(source, dict), "retained qualification omitted source identity")
    require(source == manifest["source"], "retained qualification source identity does not match package")

    package = evidence.get("package")
    require(isinstance(package, dict), "retained qualification omitted package identity")
    require(
        package.get("archive_sha256") == archive_sha256,
        "retained qualification names a different archive",
    )
    require(
        package.get("executable_sha256") == manifest["binary"]["sha256"],
        "retained qualification names a different executable",
    )
    require(
        package.get("asset_target") == manifest["asset_target"],
        "retained qualification names a different package target",
    )
    require(
        package.get("release_version") == manifest["release_version"],
        "retained qualification names a different release version",
    )
    require(
        package.get("model_sha256") == manifest["model"]["sha256"],
        "retained qualification names a different model",
    )
    require(
        package.get("matrix_cell_id") == required_matrix_cell_id
        and package.get("policy") == expected_policy
        and normalized_backend(package.get("backend"))
        == normalized_backend(expected_backend)
        and package.get("accelerator_claim") == expected_accelerator_claim,
        "retained qualification package does not match the requested matrix cell, policy, backend, or accelerator claim",
    )
    for field in (
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    ):
        require(
            package.get(field) == manifest["server_proof"][field],
            f"retained qualification {field} does not match package",
        )

    host = evidence.get("host")
    require(isinstance(host, dict), "retained qualification omitted host identity")
    require_sha256(host.get("fingerprint"), "retained qualification host fingerprint")
    require_nonempty_string(host.get("platform"), "retained qualification host platform")
    require(
        host.get("target") == manifest["asset_target"],
        "retained qualification host names a different package target",
    )
    require_nonempty_string(host.get("backend"), "retained qualification host backend")
    require(
        host.get("matrix_cell_id") == required_matrix_cell_id
        and host.get("accelerator_claim") == expected_accelerator_claim
        and host.get("host_class") == matrix_cell["host_class"],
        "retained qualification host does not match the requested matrix cell",
    )
    require(
        normalized_backend(package.get("backend")) == normalized_backend(host["backend"]),
        "retained qualification package and host backend identities disagree",
    )
    require(
        package.get("policy") == host.get("policy")
        and host.get("policy") in {"accelerated", "cpu_explicit"},
        "retained qualification package and host policy identities disagree",
    )
    require(
        host.get("policy") == expected_policy
        and normalized_backend(host.get("backend"))
        == normalized_backend(expected_backend),
        "retained qualification host used the wrong requested policy or backend",
    )
    for field in ("cache_state", "residency_state"):
        require_nonempty_string(host.get(field), f"retained qualification host {field}")
        require(
            package.get(field) == host[field],
            f"retained qualification package and host {field} disagree",
        )
        require(
            host[field] == matrix_cell[field],
            f"retained qualification host {field} differs from the selected matrix cell",
        )
    require(
        host.get("unplanned_suspend") is False,
        "retained qualification host recorded an unplanned suspend",
    )

    same_account = evidence.get("same_account")
    require(isinstance(same_account, dict), "retained qualification omitted same-account evidence")
    require_nonempty_string(same_account.get("account_id"), "same_account.account_id")
    require(
        same_account.get("relation") == "same_os_account",
        "retained qualification does not prove same-OS-account scope",
    )
    hosts = same_account.get("plugin_hosts")
    require(isinstance(hosts, list) and len(hosts) == 2, "qualification requires exactly two plugin hosts")
    host_ids: set[tuple[object, object]] = set()
    repository_ids: set[str] = set()
    for index, host in enumerate(hosts):
        require(isinstance(host, dict), f"plugin host {index} is malformed")
        require_positive_int(host.get("pid"), f"plugin_hosts[{index}].pid")
        start_id = require_nonempty_string(
            host.get("process_start_id"),
            f"plugin_hosts[{index}].process_start_id",
        )
        repository_id = require_nonempty_string(
            host.get("repository_id"),
            f"plugin_hosts[{index}].repository_id",
        )
        require(
            not Path(repository_id).is_absolute(),
            "retained plugin-host evidence must use an opaque repository identity, not a path",
        )
        host_ids.add((host["pid"], start_id))
        repository_ids.add(repository_id)
    require(len(host_ids) == 2, "plugin hosts are not independently started processes")
    require(len(repository_ids) == 2, "plugin hosts did not use different repositories")
    require(
        same_account.get("cross_login_or_terminal_sessions_proven") is False,
        "base same-account evidence must not infer cross-session sharing",
    )

    retained_shared = evidence.get("shared_identity")
    require(isinstance(retained_shared, dict), "retained qualification omitted shared server identity")
    require(
        isinstance(shared_identity, dict),
        "live two-host proof omitted shared server identity",
    )
    for field in (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "server_instance_id",
        "server_process_start_id",
        "engine_owner_id",
        "native_worker_id",
        "load_generation",
        "model_load_count",
    ):
        require(field in retained_shared, f"retained shared identity omitted {field}")
        require(
            retained_shared[field] == shared_identity[field],
            f"retained shared identity {field} does not match the live two-host proof",
        )
    require(retained_shared["model_load_count"] == 1, "retained cold race did not prove one model load")

    timing = evidence.get("timing")
    require(isinstance(timing, dict), "retained qualification omitted timing identity")
    require(timing.get("clock_domain") == "awake_monotonic", "qualification used the wrong clock domain")
    require(timing.get("cross_process_timestamp_subtraction") is False, "qualification subtracted cross-process timestamps")
    require(timing.get("unplanned_suspend") is False, "qualification performance block included suspend")
    require(timing.get("constants_frozen_before_run") is True, "qualification selected constants from its own results")
    require(
        timing.get("constant_set_sha256") == manifest["server_proof"]["constant_set_sha256"],
        "qualification timing used a different constant set",
    )

    scenarios = evidence.get("scenarios")
    require(isinstance(scenarios, dict), "retained qualification omitted scenario evidence")
    scenario_contracts = measurement_contract["measurement_protocol"]["scenario_contracts"]
    require(set(scenarios) == REQUIRED_SERVER_SCENARIOS, "retained qualification scenario set is incomplete")
    for scenario_id in sorted(REQUIRED_SERVER_SCENARIOS):
        scenario = scenarios.get(scenario_id)
        require(isinstance(scenario, dict), f"scenario {scenario_id} is malformed")
        require(scenario.get("status") == "pass", f"scenario {scenario_id} did not pass")
        assertions = scenario.get("assertions")
        require(isinstance(assertions, dict), f"scenario {scenario_id} omitted assertions")
        required_assertions = set(scenario_contracts[scenario_id]["required"])
        require(
            set(assertions) == required_assertions,
            f"scenario {scenario_id} assertions do not match the preregistered contract",
        )
        failed = sorted(name for name, passed in assertions.items() if passed is not True)
        require(not failed, f"scenario {scenario_id} has failed assertions: " + ", ".join(failed))
        artifacts = scenario.get("artifacts")
        require(isinstance(artifacts, list) and artifacts, f"scenario {scenario_id} has no retained artifacts")
        artifact_names: set[str] = set()
        for artifact in artifacts:
            require(isinstance(artifact, dict), f"scenario {scenario_id} artifact is malformed")
            name = require_nonempty_string(artifact.get("name"), f"scenario {scenario_id} artifact name")
            require(
                Path(name).name == name and Path(name).suffix == ".json",
                f"scenario {scenario_id} artifact name is not a safe JSON basename",
            )
            require_sha256(artifact.get("sha256"), f"scenario {scenario_id} artifact sha256")
            artifact_names.add(name)
        if scenario_id in {"server_crash", "worker_stall"}:
            require(
                "publication-fault-external.raw.json" in artifact_names,
                f"{scenario_id} scenario omitted separately hashed publication-fence evidence",
            )

    nonclaims = evidence.get("lower_tier_nonclaims")
    require(isinstance(nonclaims, dict), "retained qualification omitted lower-tier nonclaims")
    require(set(nonclaims) == LOWER_TIER_NONCLAIMS, "retained qualification nonclaim set is incomplete")
    for claim, record in nonclaims.items():
        require(isinstance(record, dict), f"nonclaim {claim} is malformed")
        require(record.get("claimed") is False, f"lower-tier evidence incorrectly claims {claim}")
        require_nonempty_string(record.get("reason"), f"nonclaim {claim} reason")

    metrics = evidence.get("metrics")
    require(isinstance(metrics, dict), "retained qualification omitted metric results")
    required_metrics = set(measurement_contract["measurement_protocol"]["required_metrics"])
    require(set(metrics) == required_metrics, "retained qualification metric set is incomplete")
    thresholds = measurement_contract["constant_set"]["qualification_thresholds"]
    metric_contracts = measurement_contract["measurement_protocol"]["metric_contracts"]
    for metric, result in metrics.items():
        require(isinstance(result, dict), f"metric {metric} is malformed")
        require(result.get("status") == "pass", f"metric {metric} did not pass its frozen threshold")
        require(
            result.get("unit") == metric_contracts[metric]["unit"],
            f"metric {metric} used the wrong unit",
        )
        require(
            isinstance(result.get("value"), (int, float))
            and not isinstance(result.get("value"), bool),
            f"metric {metric} value is not numeric",
        )
        require(
            result.get("threshold") == thresholds[metric]
            and isinstance(result.get("threshold"), (int, float))
            and not isinstance(result.get("threshold"), bool),
            f"metric {metric} threshold does not match the frozen constant set",
        )
        comparison = metric_contracts[metric]["comparison"]
        require(result.get("comparison") == comparison, f"metric {metric} used the wrong comparison")
        if metric == "retrieval_quality":
            raw_evidence = result.get("raw_evidence")
            require(isinstance(raw_evidence, dict), "retrieval quality metric omitted raw evidence")
            require_exact_keys(
                raw_evidence,
                {
                    "artifact",
                    "evaluation_contract",
                    "source_commit",
                    "source_tree",
                    "corpus_id",
                    "holdout_manifest_set_sha256",
                    "repeats",
                    "row_count",
                    "passing_row_count",
                    "publishable_packet_pass_rate",
                },
                "retrieval quality retained raw evidence",
            )
            artifact = raw_evidence["artifact"]
            require(isinstance(artifact, dict), "retrieval quality raw artifact is malformed")
            require_exact_keys(artifact, {"name", "sha256"}, "retrieval quality raw artifact")
            require(
                artifact["name"] == "packet-runtime-summary.json",
                "retrieval quality raw artifact name is invalid",
            )
            require_sha256(artifact["sha256"], "retrieval quality raw artifact sha256")
            require(
                raw_evidence["evaluation_contract"] == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
                "retrieval quality retained evaluation contract changed",
            )
            require(
                raw_evidence["source_commit"] == evidence["source"]["commit"]
                and raw_evidence["source_tree"] == evidence["source"]["tree"],
                "retrieval quality retained source identity is stale",
            )
            require(
                require_positive_int(raw_evidence["repeats"], "retrieval quality repeats")
                == MIN_RETRIEVAL_QUALITY_REPEATS,
                "retrieval quality retained the wrong repeat count",
            )
            require(
                raw_evidence["corpus_id"] == RELEASE_QUALITY_CORPUS_ID,
                "retrieval quality retained the wrong holdout corpus",
            )
            require_sha256(
                raw_evidence["holdout_manifest_set_sha256"],
                "retrieval quality holdout manifest set sha256",
            )
            row_count = require_positive_int(
                raw_evidence["row_count"], "retrieval quality row count"
            )
            require(
                require_positive_int(
                    raw_evidence["passing_row_count"],
                    "retrieval quality passing row count",
                )
                == row_count,
                "retrieval quality retained a failing row",
            )
            require(
                isinstance(raw_evidence["publishable_packet_pass_rate"], (int, float))
                and not isinstance(raw_evidence["publishable_packet_pass_rate"], bool),
                "retrieval quality pass rate is not numeric",
            )
            require(
                raw_evidence["publishable_packet_pass_rate"] == result["value"],
                "retrieval quality metric does not match its raw evidence",
            )
        else:
            raw_evidence = result.get("raw_evidence")
            require(
                isinstance(raw_evidence, dict),
                f"metric {metric} omitted its raw measurement artifact",
            )
            require_exact_keys(
                raw_evidence,
                {"name", "sha256"},
                f"metric {metric} raw measurement artifact",
            )
            expected_artifact_name = (
                "total-codestory-process-memory.raw.json"
                if metric == "total_codestory_process_memory"
                else "measurements.raw.json"
            )
            require(
                raw_evidence["name"] == expected_artifact_name,
                f"metric {metric} used the wrong raw measurement artifact",
            )
            require_sha256(raw_evidence["sha256"], f"metric {metric} raw artifact sha256")
        passed = {
            "equal": result["value"] == result["threshold"],
            "greater_than_or_equal": result["value"] >= result["threshold"],
            "less_than_or_equal": result["value"] <= result["threshold"],
        }[comparison]
        require(passed, f"metric {metric} value failed its frozen comparison")

    return evidence


def parse_byte_quantity(value: str) -> int:
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)([KMG])?", value.strip())
    require(match is not None, f"invalid memory quantity: {value!r}")
    scale = {None: 1, "K": 1024, "M": 1024**2, "G": 1024**3}[match.group(2)]
    return round(float(match.group(1)) * scale)


def process_resident_memory(pid: int) -> tuple[int, str]:
    if os.name == "nt":
        command = [
            "powershell",
            "-NoProfile",
            "-Command",
            f"(Get-Process -Id {pid} -ErrorAction Stop).WorkingSet64",
        ]
        scale = 1
        metric = "windows_working_set"
    elif sys.platform == "darwin":
        completed = subprocess.run(
            ["vmmap", "-summary", str(pid)],
            text=True,
            capture_output=True,
            timeout=20,
        )
        require(completed.returncode == 0, f"could not read physical footprint for process {pid}: {completed.stderr.strip()}")
        match = re.search(r"^Physical footprint:\s+([^\s]+)", completed.stdout, re.MULTILINE)
        require(match is not None, f"vmmap omitted the physical footprint for process {pid}")
        return parse_byte_quantity(match.group(1)), "macos_physical_footprint"
    else:
        command = ["ps", "-o", "rss=", "-p", str(pid)]
        scale = 1024
        metric = "rss"
    completed = subprocess.run(command, text=True, capture_output=True, timeout=10)
    require(completed.returncode == 0, f"could not read RSS for process {pid}: {completed.stderr.strip()}")
    try:
        return int(completed.stdout.strip()) * scale, metric
    except ValueError as exc:
        raise ProofFailure(f"invalid RSS for process {pid}: {completed.stdout!r}") from exc


def suspend_clock_pair(target_os: str) -> tuple[int, int, str, str]:
    awake_ns = time.monotonic_ns()
    if target_os == "linux":
        require(
            hasattr(time, "CLOCK_BOOTTIME"),
            "Linux qualification host lacks CLOCK_BOOTTIME",
        )
        inclusive_ns = time.clock_gettime_ns(time.CLOCK_BOOTTIME)
        return awake_ns, inclusive_ns, "CLOCK_MONOTONIC", "CLOCK_BOOTTIME"
    if target_os == "macos":
        class MachTimebaseInfo(ctypes.Structure):
            _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]

        system = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        system.mach_continuous_time.restype = ctypes.c_uint64
        system.mach_timebase_info.argtypes = [ctypes.POINTER(MachTimebaseInfo)]
        info = MachTimebaseInfo()
        require(
            system.mach_timebase_info(ctypes.byref(info)) == 0 and info.denom > 0,
            "macOS qualification host could not read mach timebase",
        )
        inclusive_ticks = system.mach_continuous_time()
        inclusive_ns = inclusive_ticks * info.numer // info.denom
        return awake_ns, inclusive_ns, "mach_absolute_time", "mach_continuous_time"
    require(target_os == "windows", f"unsupported qualification clock target {target_os}")
    kernel = ctypes.windll.kernel32
    unbiased = ctypes.c_ulonglong()
    inclusive = ctypes.c_ulonglong()
    require(
        bool(kernel.QueryUnbiasedInterruptTimePrecise(ctypes.byref(unbiased))),
        "Windows qualification host could not read unbiased interrupt time",
    )
    kernel.QueryInterruptTimePrecise(ctypes.byref(inclusive))
    return (
        int(unbiased.value) * 100,
        int(inclusive.value) * 100,
        "QueryUnbiasedInterruptTimePrecise",
        "QueryInterruptTimePrecise",
    )


def plugin_client_process(
    status: dict,
    manifest: dict,
    label: str,
    *,
    target_os: str,
) -> dict:
    plugin_runtime = status.get("plugin_runtime")
    require(isinstance(plugin_runtime, dict), f"{label} omitted plugin_runtime")
    process = plugin_runtime.get("client_process")
    require(isinstance(process, dict), f"{label} omitted client_process")
    require_exact_keys(
        process,
        {"pid", "process_start_id", "executable_sha256"},
        f"{label} client_process",
    )
    pid = require_positive_int(process["pid"], f"{label} client_process.pid")
    start_id = require_nonempty_string(
        process["process_start_id"],
        f"{label} client_process.process_start_id",
    )
    return verified_live_executable(
        pid=pid,
        process_start_id=start_id,
        reported_sha256=process["executable_sha256"],
        expected_sha256=manifest["binary"]["sha256"],
        target_os=target_os,
        label=f"{label} client process",
    )


def capture_five_process_memory(
    *,
    args: argparse.Namespace,
    node_path: Path,
    host_a: McpProcess,
    host_a_start: str,
    host_b: McpProcess,
    host_b_start: str,
    status_a: dict,
    status_b: dict,
    snapshot: dict,
    manifest: dict,
    expected_backend: str,
) -> dict:
    protocol, _ = load_measurement_protocol(args.measurement_protocol)
    matrix_cell_id = require_nonempty_string(
        args.qualification_matrix_cell,
        "memory qualification requires --qualification-matrix-cell",
    )
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=manifest["asset_target"],
        proof_tier=args.proof_tier,
        expected_policy=args.engine_policy,
        expected_backend=expected_backend,
    )
    target_os = TARGET_CONTRACTS[manifest["asset_target"]]["target_os"]
    client_a = plugin_client_process(
        status_a,
        manifest,
        "first plugin host",
        target_os=target_os,
    )
    client_b = plugin_client_process(
        status_b,
        manifest,
        "second plugin host",
        target_os=target_os,
    )
    require(
        (client_a["pid"], client_a["process_start_id"])
        != (client_b["pid"], client_b["process_start_id"]),
        "plugin hosts reported the same CLI client process",
    )
    server = snapshot["process"]
    server_live = verified_live_executable(
        pid=require_positive_int(server.get("pid"), "embedding server pid"),
        process_start_id=require_nonempty_string(
            server.get("process_start_id"),
            "embedding server process_start_id",
        ),
        reported_sha256=server.get("executable_sha256"),
        expected_sha256=manifest["binary"]["sha256"],
        target_os=target_os,
        label="embedding server process",
    )
    node_digest = sha256(node_path.resolve())
    process_set = [
        {
            "role": "plugin_host_a",
            "pid": host_a.process.pid,
            "process_start_id": host_a_start,
            "executable_sha256": node_digest,
        },
        {"role": "plugin_cli_a", **client_a},
        {
            "role": "plugin_host_b",
            "pid": host_b.process.pid,
            "process_start_id": host_b_start,
            "executable_sha256": node_digest,
        },
        {"role": "plugin_cli_b", **client_b},
        {
            "role": "embedding_server",
            **server_live,
        },
    ]
    identities = {
        (process["pid"], process["process_start_id"]) for process in process_set
    }
    require(
        len(identities) == 5,
        "memory evidence did not identify five distinct live CodeStory processes",
    )
    boot_id = require_nonempty_string(
        snapshot["clock"]["boot_id"],
        "embedding server clock boot_id",
    )
    samples = []
    for repeat in range(1, 4):
        awake_started, inclusive_started, awake_api, inclusive_api = (
            suspend_clock_pair(target_os)
        )
        processes = []
        for process in process_set:
            require(
                process_start_identity(process["pid"])
                == process["process_start_id"],
                f"memory process {process['role']} changed identity before sampling",
            )
            resident_bytes, measurement_api = process_resident_memory(process["pid"])
            processes.append(
                {
                    **process,
                    "resident_bytes": resident_bytes,
                    "measurement_api": measurement_api,
                }
            )
        awake_finished, inclusive_finished, finished_awake_api, finished_inclusive_api = (
            suspend_clock_pair(target_os)
        )
        require(
            finished_awake_api == awake_api
            and finished_inclusive_api == inclusive_api,
            "memory sampling clock API changed within one sample",
        )
        for process in process_set:
            require(
                process_start_identity(process["pid"])
                == process["process_start_id"],
                f"memory process {process['role']} changed identity during sampling",
            )
        samples.append(
            {
                "sample_id": canonical_sha256(
                    {
                        "matrix_cell_id": matrix_cell_id,
                        "repeat": repeat,
                        "identities": sorted(identities),
                    }
                ),
                "repeat": repeat,
                "matrix_cell_id": matrix_cell_id,
                "workload_id": protocol["workloads"][
                    "total_codestory_process_memory"
                ]["workload_id"],
                "cache_state": matrix_cell["cache_state"],
                "residency_state": matrix_cell["residency_state"],
                "producer_process": {
                    "pid": os.getpid(),
                    "process_start_id": process_start_identity(os.getpid()),
                },
                "server_identity": {
                    "server_instance_id": snapshot["process"]["server_instance_id"],
                    "process_start_id": snapshot["process"]["process_start_id"],
                    "load_generation": snapshot["engine"]["load_generation"],
                },
                "clock": {
                    "domain": "awake_monotonic",
                    "api": awake_api,
                    "boot_id": boot_id,
                    "resolution_ns": max(1, round(time.get_clock_info("monotonic").resolution * 1e9)),
                },
                "start": {
                    "phase": "steady_state_process_set_identified",
                    "observed_ns": awake_started,
                },
                "end": {
                    "phase": "steady_state_memory_samples_collected",
                    "observed_ns": awake_finished,
                },
                "operands": {"processes": processes},
                "suspend_witness": {
                    "awake_started_ns": awake_started,
                    "awake_finished_ns": awake_finished,
                    "inclusive_clock_api": inclusive_api,
                    "inclusive_started_ns": inclusive_started,
                    "inclusive_finished_ns": inclusive_finished,
                    "boot_id_started": boot_id,
                    "boot_id_finished": boot_id,
                },
            }
        )
        if repeat < 3:
            time.sleep(0.25)
    return {
        "evidence_contract": MEMORY_EVIDENCE_CONTRACT,
        "metric": "total_codestory_process_memory",
        "unit": "bytes",
        "samples": samples,
    }


def retain_five_process_memory_evidence(
    artifact_root: Path,
    raw: object,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    protocol: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    require(isinstance(raw, dict), "live runtime omitted five-process memory observations")
    require_exact_keys(
        raw,
        {"evidence_contract", "metric", "unit", "samples"},
        "five-process memory observations",
    )
    payload = {
        "schema_version": 1,
        "source": source,
        "package": package,
        "contracts": contracts,
        **raw,
    }
    name = "total-codestory-process-memory.raw.json"
    path = artifact_root / name
    write_private_json(path, payload)
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            "five-process memory artifact leaked private request material",
        )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "source",
            "package",
            "contracts",
            "evidence_contract",
            "metric",
            "unit",
            "samples",
        },
        "five-process memory artifact",
    )
    require(
        payload["schema_version"] == 1
        and payload["source"] == source
        and payload["package"] == package
        and payload["contracts"] == contracts
        and payload["evidence_contract"] == MEMORY_EVIDENCE_CONTRACT
        and payload["metric"] == "total_codestory_process_memory"
        and payload["unit"] == "bytes",
        "five-process memory artifact changed its bound contract",
    )
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=target,
        proof_tier=proof_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    samples = payload["samples"]
    require(
        isinstance(samples, list) and len(samples) == 3,
        "five-process memory evidence requires three samples",
    )
    target_os = TARGET_CONTRACTS[target]["target_os"]
    clock_policy = protocol["clock_policy"]
    allowed_awake_apis = set(clock_policy["platform_apis"][target_os])
    suspend_contract = clock_policy["suspend_detection"]
    expected_measurement_api = {
        "linux": "rss",
        "macos": "macos_physical_footprint",
        "windows": "windows_working_set",
    }[target_os]
    values = []
    sample_ids: set[str] = set()
    server_identities: set[tuple[str, str, int]] = set()
    for index, sample in enumerate(samples):
        require(isinstance(sample, dict), f"five-process memory sample {index} is malformed")
        require_exact_keys(
            sample,
            {
                "sample_id",
                "repeat",
                "matrix_cell_id",
                "workload_id",
                "cache_state",
                "residency_state",
                "producer_process",
                "server_identity",
                "clock",
                "start",
                "end",
                "operands",
                "suspend_witness",
            },
            f"five-process memory sample {index}",
        )
        sample_id = require_opaque_identifier(
            sample["sample_id"],
            f"five-process memory sample {index}.sample_id",
        )
        require(sample_id not in sample_ids, "five-process memory sample id was reused")
        sample_ids.add(sample_id)
        require(
            sample["repeat"] == index + 1
            and sample["matrix_cell_id"] == matrix_cell_id
            and sample["workload_id"]
            == protocol["workloads"]["total_codestory_process_memory"]["workload_id"]
            and sample["cache_state"] == matrix_cell["cache_state"]
            and sample["residency_state"] == matrix_cell["residency_state"],
            "five-process memory sample changed its preregistered cell or workload",
        )
        processes = sample.get("operands", {}).get("processes", [])
        require(
            all(
                isinstance(process, dict)
                and process.get("measurement_api") == expected_measurement_api
                for process in processes
            ),
            "five-process memory sample used the wrong platform memory API",
        )
        package_processes = {
            "plugin_cli_a",
            "plugin_cli_b",
            "embedding_server",
        }
        require(
            all(
                process.get("executable_sha256") == package["executable_sha256"]
                for process in processes
                if process.get("role") in package_processes
            ),
            "five-process memory sample used a different packaged executable",
        )
        server = sample["server_identity"]
        require(
            isinstance(server, dict),
            f"five-process memory sample {index} server identity is malformed",
        )
        require_exact_keys(
            server,
            {"server_instance_id", "process_start_id", "load_generation"},
            f"five-process memory sample {index} server identity",
        )
        server_identities.add(
            (
                require_opaque_identifier(
                    server["server_instance_id"],
                    f"five-process memory sample {index}.server_instance_id",
                ),
                require_nonempty_string(
                    server["process_start_id"],
                    f"five-process memory sample {index}.server_process_start_id",
                ),
                require_positive_int(
                    server["load_generation"],
                    f"five-process memory sample {index}.load_generation",
                ),
            )
        )
        adapted = {**sample, "process": sample["producer_process"]}
        adapted.pop("producer_process")
        values.append(
            qualification_measurement_sample_value(
                "total_codestory_process_memory",
                adapted,
                contracts=contracts,
                phase_boundaries=protocol["phase_boundaries"],
                allowed_awake_apis=allowed_awake_apis,
                inclusive_api=suspend_contract["platform_apis"][target_os],
                maximum_suspend_ns=suspend_contract[
                    "maximum_inclusive_minus_awake_ns"
                ],
                expected_policy=expected_policy,
                expected_backend=expected_backend,
            )
        )
    require(
        len(server_identities) == 1,
        "five-process memory block changed shared server identity",
    )
    return {
        "artifact": {
            "name": name,
            "sha256": hashlib.sha256(payload_bytes).hexdigest(),
        },
        "value": max(values),
        "payload": payload,
    }


def assert_public_status(status: dict) -> None:
    require(find_value(status, "retrieval_mode") == "full", "public status does not report full retrieval")
    maintainer_only = (
        "sidecar",
        "full_repair",
        "embedding_model_sha256",
        "embedding_ggml_build_identity",
        "embedding_backend",
        "embedding_adapter",
        "embedding_policy",
        "embedding_engine_instance_id",
        "embedding_engine_residency",
        "embedding_engine_load_generation",
        "embedding_engine_load_error",
        "embedding_materialized_path",
        "embedding_detected_provider",
        "embedding_detected_gpu",
        "embedding_server",
        "server_instance_id",
        "lifetime_authority_id",
        "listener_id",
        "engine_owner_id",
        "native_worker_id",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    )
    leaked = [key for key in maintainer_only if find_value(status, key) is not None]
    require(not leaked, "public status leaked maintainer-only retrieval fields: " + ", ".join(leaked))


def extract_resource(response: dict, uri: str) -> dict:
    require("error" not in response, f"status resource failed: {response.get('error')}")
    contents = response.get("result", {}).get("contents", [])
    for item in contents:
        if isinstance(item, dict) and item.get("uri") == uri:
            payload = json.loads(item.get("text", "{}"))
            require(isinstance(payload, dict), "status resource emitted non-object JSON")
            return payload
    raise ProofFailure(f"resource response did not contain {uri}")


class McpProcess:
    def __init__(self, command: list[str], *, env: dict[str, str], cwd: Path, timeout: int):
        self.timeout = timeout
        self.process = subprocess.Popen(command, cwd=cwd, env=env, text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.stderr: list[str] = []
        assert self.process.stdout and self.process.stderr and self.process.stdin
        threading.Thread(target=self._reader, args=(self.process.stdout, self.lines), daemon=True).start()
        threading.Thread(target=self._stderr_reader, daemon=True).start()
        self.transcript: list[dict] = []
        self.tool_attempt_counts: dict[str, int] = {}

    @staticmethod
    def _reader(stream, output: queue.Queue[str | None]) -> None:
        for line in stream:
            output.put(line)
        output.put(None)

    def _stderr_reader(self) -> None:
        assert self.process.stderr
        self.stderr.extend(self.process.stderr.readlines())

    def send(self, request: dict) -> dict:
        assert self.process.stdin
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"MCP request timed out: {request.get('id')}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty as exc:
                raise ProofFailure(f"MCP request timed out: {request.get('id')}") from exc
            require(line is not None, f"MCP process closed: {''.join(self.stderr)[-2000:]}")
            response = json.loads(line)
            self.transcript.append({"request": request, "response": response})
            if response.get("id") == request.get("id"):
                return response

    def initialize(self) -> None:
        response = self.send({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "packaged-proof", "version": "1"}},
        })
        require("error" not in response, f"MCP initialize failed: {response.get('error')}")
        assert self.process.stdin
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.process.stdin.flush()

    def status(self, project: Path, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": STATUS_URI, "project": str(project)},
        }), STATUS_URI)

    def engine_diagnostics(self, project: Path, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": ENGINE_DIAGNOSTICS_URI, "project": str(project)},
        }), ENGINE_DIAGNOSTICS_URI)

    def tool(self, name: str, arguments: dict, request_id: str) -> dict:
        response = self.send({"jsonrpc": "2.0", "id": request_id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
        require("error" not in response, f"MCP {name} failed: {response.get('error')}")
        return response

    def tool_until_ready(self, name: str, arguments: dict, request_id: str) -> tuple[dict, int]:
        deadline = time.monotonic() + self.timeout
        attempt = 0
        while True:
            attempt += 1
            self.tool_attempt_counts[request_id] = attempt
            response = self.tool(name, arguments, f"{request_id}-{attempt}")
            result = response.get("result")
            require(
                isinstance(result, dict),
                f"MCP {name} attempt {attempt} returned a non-object result: {result!r}",
            )
            state = result.get("structuredContent")
            require(
                isinstance(state, dict),
                f"MCP {name} attempt {attempt} returned non-object structuredContent: {result!r}",
            )
            is_error = result.get("isError")
            if "isError" not in result or is_error is False:
                return response, attempt
            require(
                is_error is True,
                f"MCP {name} attempt {attempt} returned invalid isError={is_error!r}: {result!r}",
            )
            retry_state = (state.get("code"), state.get("state"))
            require(
                retry_state
                in (
                    ("codestory_preparing", "preparing"),
                    ("codestory_updating", "updating"),
                ),
                f"MCP {name} attempt {attempt} returned a terminal or malformed error envelope: {state!r}",
            )
            require(
                state.get("retry_tool") == name,
                f"MCP {name} attempt {attempt} returned the wrong retry tool: {state!r}",
            )
            retry_after_ms = state.get("retry_after_ms")
            require(
                isinstance(retry_after_ms, int)
                and not isinstance(retry_after_ms, bool)
                and retry_after_ms >= 0,
                f"MCP {name} attempt {attempt} returned invalid retry_after_ms: {state!r}",
            )
            remaining = deadline - time.monotonic()
            require(
                remaining > 0,
                f"MCP {name} did not become ready after attempt {attempt}: {state!r}",
            )
            delay_ms = min(retry_after_ms, max(0, int(remaining * 1000)))
            time.sleep(delay_ms / 1000)

    def search_until_ready(self, arguments: dict, request_id: str) -> tuple[dict, int]:
        response, attempts = self.tool_until_ready("search", arguments, request_id)
        result = response["result"]
        state = result["structuredContent"]
        query = arguments.get("query")
        require(
            isinstance(query, str) and state.get("query") == query,
            f"MCP search returned a mismatched query: expected {query!r}, response={state!r}",
        )
        require(
            isinstance(state.get("hits"), list),
            f"MCP search returned non-array hits: {state!r}",
        )
        retrieval = state.get("retrieval")
        require(
            isinstance(retrieval, dict) and retrieval.get("state") == "ready",
            f"MCP search did not return the ready installed retrieval projection: {state!r}",
        )
        # The installed result is deliberately compact. Full retrieval remains
        # proven separately by public status and activation diagnostics.
        return response, attempts

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)


def process_start_identity(pid: int) -> str:
    if os.name == "nt":
        class FileTime(ctypes.Structure):
            _fields_ = [
                ("low_date_time", ctypes.c_uint32),
                ("high_date_time", ctypes.c_uint32),
            ]

        kernel = ctypes.windll.kernel32
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.GetProcessTimes.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
        ]
        kernel.GetProcessTimes.restype = ctypes.c_int
        kernel.GetExitCodeProcess.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        kernel.GetExitCodeProcess.restype = ctypes.c_int
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x1000, 0, pid)
        require(bool(handle), f"could not open process {pid} for start identity")
        try:
            creation = FileTime()
            exit_time = FileTime()
            kernel_time = FileTime()
            user_time = FileTime()
            require(
                bool(
                    kernel.GetProcessTimes(
                        handle,
                        ctypes.byref(creation),
                        ctypes.byref(exit_time),
                        ctypes.byref(kernel_time),
                        ctypes.byref(user_time),
                    )
                ),
                f"could not read process start identity for {pid}",
            )
            exit_code = ctypes.c_uint32()
            require(
                bool(kernel.GetExitCodeProcess(handle, ctypes.byref(exit_code)))
                and exit_code.value == 259
                and exit_time.low_date_time == 0
                and exit_time.high_date_time == 0,
                f"process {pid} was not running during start-identity inspection",
            )
        finally:
            kernel.CloseHandle(handle)
        filetime_ticks = (creation.high_date_time << 32) | creation.low_date_time
        # Match codestory-retrieval's legacy DateTime-tick serialization exactly.
        creation_ticks = (filetime_ticks // 10 * 10) + 504_911_232_000_000_000
        return f"windows:{creation_ticks}"
    if sys.platform == "linux":
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        fields = stat.rsplit(") ", 1)
        require(len(fields) == 2, f"/proc/{pid}/stat omitted process start identity")
        process_fields = fields[1].split()
        require(len(process_fields) > 19, f"/proc/{pid}/stat omitted process start identity")
        return f"linux:{process_fields[19]}"
    if sys.platform == "darwin":
        class ProcBsdInfo(ctypes.Structure):
            _fields_ = [
                ("pbi_flags", ctypes.c_uint32),
                ("pbi_status", ctypes.c_uint32),
                ("pbi_xstatus", ctypes.c_uint32),
                ("pbi_pid", ctypes.c_uint32),
                ("pbi_ppid", ctypes.c_uint32),
                ("pbi_uid", ctypes.c_uint32),
                ("pbi_gid", ctypes.c_uint32),
                ("pbi_ruid", ctypes.c_uint32),
                ("pbi_rgid", ctypes.c_uint32),
                ("pbi_svuid", ctypes.c_uint32),
                ("pbi_svgid", ctypes.c_uint32),
                ("rfu_1", ctypes.c_uint32),
                ("pbi_comm", ctypes.c_char * 16),
                ("pbi_name", ctypes.c_char * 32),
                ("pbi_nfiles", ctypes.c_uint32),
                ("pbi_pgid", ctypes.c_uint32),
                ("pbi_pjobc", ctypes.c_uint32),
                ("e_tdev", ctypes.c_uint32),
                ("e_tpgid", ctypes.c_uint32),
                ("pbi_nice", ctypes.c_int32),
                ("pbi_start_tvsec", ctypes.c_uint64),
                ("pbi_start_tvusec", ctypes.c_uint64),
            ]

        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        libproc.proc_pidinfo.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_int,
        ]
        libproc.proc_pidinfo.restype = ctypes.c_int
        info = ProcBsdInfo()
        expected = ctypes.sizeof(info)
        read = libproc.proc_pidinfo(pid, 3, 0, ctypes.byref(info), expected)
        require(
            read == expected and info.pbi_pid == pid,
            f"could not read complete process start identity for {pid}",
        )
        return f"macos-proc:{info.pbi_start_tvsec}:{info.pbi_start_tvusec}"
    completed = subprocess.run(
        ["ps", "-o", "lstart=", "-p", str(pid)],
        text=True,
        capture_output=True,
        timeout=20,
    )
    require(completed.returncode == 0, f"could not read process start identity for {pid}")
    return "unix:" + require_nonempty_string(
        completed.stdout.strip(), "process start identity"
    )


def require_native_process_start_identity(
    identity: object,
    target_os: str,
    label: str,
) -> str:
    value = require_nonempty_string(identity, label)
    patterns = {
        "linux": r"linux:[0-9]+",
        "macos": r"macos-proc:[0-9]+:[0-9]+",
        "windows": r"windows:[0-9]+",
    }
    require(target_os in patterns, f"{label} used unsupported target OS {target_os}")
    require(
        re.fullmatch(patterns[target_os], value) is not None,
        f"{label} did not use the canonical {target_os} process identity format",
    )
    return value


def live_process_executable_sha256(
    pid: int,
    expected_start_id: str,
    target_os: str,
) -> str:
    expected_start_id = require_native_process_start_identity(
        expected_start_id,
        target_os,
        f"process {pid} expected start identity",
    )
    require(
        process_start_identity(pid) == expected_start_id,
        f"process {pid} changed identity before executable-image inspection",
    )
    if target_os == "linux":
        descriptor = os.open(f"/proc/{pid}/exe", os.O_RDONLY)
    elif target_os == "macos":
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
        libproc.proc_pidpath.argtypes = [
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_uint32,
        ]
        libproc.proc_pidpath.restype = ctypes.c_int
        buffer = ctypes.create_string_buffer(4096)
        length = libproc.proc_pidpath(pid, buffer, len(buffer))
        require(length > 0, f"proc_pidpath could not inspect process {pid}")
        executable_path = os.fsdecode(buffer.raw[:length].split(b"\0", 1)[0])
        descriptor = os.open(executable_path, os.O_RDONLY)
    else:
        require(target_os == "windows", f"unsupported executable-image target {target_os}")
        kernel = ctypes.windll.kernel32
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.QueryFullProcessImageNameW.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.c_wchar_p,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        kernel.QueryFullProcessImageNameW.restype = ctypes.c_int
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x1000, 0, pid)
        require(bool(handle), f"OpenProcess could not inspect process {pid}")
        try:
            buffer = ctypes.create_unicode_buffer(32768)
            length = ctypes.c_uint32(len(buffer))
            require(
                bool(
                    kernel.QueryFullProcessImageNameW(
                        handle,
                        0,
                        buffer,
                        ctypes.byref(length),
                    )
                ),
                f"QueryFullProcessImageNameW could not inspect process {pid}",
            )
            executable_path = buffer.value[: length.value]
        finally:
            kernel.CloseHandle(handle)
        descriptor = os.open(executable_path, os.O_RDONLY | getattr(os, "O_BINARY", 0))
    digest = hashlib.sha256()
    try:
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    finally:
        os.close(descriptor)
    require(
        process_start_identity(pid) == expected_start_id,
        f"process {pid} changed identity during executable-image inspection",
    )
    return digest.hexdigest()


def verified_live_executable(
    *,
    pid: int,
    process_start_id: str,
    reported_sha256: str,
    expected_sha256: str,
    target_os: str,
    label: str,
) -> dict:
    require_sha256(reported_sha256, f"{label} reported executable sha256")
    require_sha256(expected_sha256, f"{label} expected executable sha256")
    live_sha256 = live_process_executable_sha256(pid, process_start_id, target_os)
    require(
        live_sha256 == reported_sha256 == expected_sha256,
        f"{label} live executable image does not match its reported and packaged digest",
    )
    return {
        "pid": pid,
        "process_start_id": process_start_id,
        "executable_sha256": live_sha256,
    }


def current_account_identity() -> str:
    if os.name != "nt":
        raw = f"uid:{os.geteuid()}"
        return "account:" + hashlib.sha256(raw.encode("utf-8")).hexdigest()
    completed = subprocess.run(
        ["whoami", "/user", "/fo", "csv", "/nh"],
        text=True,
        capture_output=True,
        timeout=20,
    )
    require(completed.returncode == 0, "could not read current Windows account SID")
    match = re.search(r'"(S-[0-9-]+)"\s*$', completed.stdout.strip())
    require(match is not None, "Windows account command omitted SID")
    raw = f"sid:{match.group(1)}"
    return "account:" + hashlib.sha256(raw.encode("utf-8")).hexdigest()


def opaque_repository_id(project: Path) -> str:
    return "repo:" + hashlib.sha256(str(project.resolve()).encode("utf-8")).hexdigest()


def directory_contract_sha256(root: Path) -> str:
    require(root.is_dir(), f"plugin package root does not exist: {root}")
    digest = hashlib.sha256()
    files = sorted(path for path in root.rglob("*") if path.is_file())
    require(files, "plugin package root is empty")
    for path in files:
        require(not path.is_symlink(), f"installed plugin package contains a symlink: {path}")
        relative = path.relative_to(root).as_posix().encode("utf-8")
        payload = path.read_bytes()
        digest.update(len(relative).to_bytes(8, "little"))
        digest.update(relative)
        digest.update(len(payload).to_bytes(8, "little"))
        digest.update(payload)
    return digest.hexdigest()


def prepare_candidate_installed_proof(args: argparse.Namespace) -> dict:
    require(
        args.archive is not None
        and args.checksum_file is not None
        and args.expected_version is not None
        and args.plugin_root is not None
        and args.candidate_plugin_root_output is not None
        and args.candidate_plugin_data_output is not None
        and args.installed_plugin_provenance_output is not None,
        "candidate install preparation requires archive, checksum, version, plugin source, "
        "plugin/data outputs, and provenance output",
    )
    archive = args.archive.resolve()
    checksum = args.checksum_file.resolve()
    source_plugin = args.plugin_root.resolve()
    plugin_output = args.candidate_plugin_root_output.resolve()
    data_output = args.candidate_plugin_data_output.resolve()
    provenance_output = args.installed_plugin_provenance_output.resolve()
    producer = {
        "repository": args.candidate_producer_repository,
        "workflow_path": args.candidate_producer_workflow_path,
        "run_id": args.candidate_producer_run_id,
        "run_attempt": args.candidate_producer_run_attempt,
        "artifact_name": args.candidate_artifact_name,
    }
    require(
        producer["repository"] == "TheGreenCedar/CodeStory"
        and producer["workflow_path"]
        == ".github/workflows/packaged-platform-pr.yml"
        and isinstance(producer["run_id"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
        and isinstance(producer["run_attempt"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"]) is not None
        and producer["artifact_name"] == archive.name,
        "candidate install producer identity is missing or is not the trusted coordinator artifact",
    )
    require(
        sha256(archive) == expected_archive_digest(checksum, archive),
        "candidate install archive checksum mismatch",
    )
    require(
        source_plugin.is_dir()
        and not plugin_output.exists()
        and not data_output.exists()
        and not provenance_output.exists(),
        "candidate install outputs must be absent and the source plugin must exist",
    )
    with tempfile.TemporaryDirectory(prefix="codestory-candidate-install-") as raw:
        unpacked = Path(raw) / "unpacked"
        unpack_archive(archive, unpacked)
        cli = find_cli(unpacked)
        manifest = load_native_manifest(unpacked, cli, args.expected_version)
        repository_root = Path(__file__).resolve().parents[2]
        require(
            os.path.samefile(
                source_plugin,
                repository_root / "plugins" / "codestory",
            ),
            "candidate install plugin source is not the checked-in CodeStory plugin",
        )

        def git(*arguments: str) -> str:
            completed = subprocess.run(
                ["git", *arguments],
                cwd=repository_root,
                text=True,
                capture_output=True,
                timeout=30,
            )
            require(
                completed.returncode == 0,
                f"candidate install Git identity probe failed: {completed.stderr.strip()}",
            )
            return completed.stdout.strip()

        require(
            git("rev-parse", "HEAD") == manifest["source"]["commit"]
            and git("rev-parse", "HEAD^{tree}") == manifest["source"]["tree"],
            "candidate plugin checkout does not match the packaged source commit and tree",
        )
        require(
            git("status", "--porcelain", "--untracked-files=all") == "",
            "candidate plugin checkout contains tracked or untracked source drift",
        )
        shutil.copytree(source_plugin, plugin_output)
        expected_archive_name = (
            f"codestory-cli-v{args.expected_version}-"
            f"{manifest['asset_target']}."
            f"{'zip' if manifest['asset_target'].startswith('windows-') else 'tar.gz'}"
        )
        require(
            archive.name == expected_archive_name,
            "candidate install archive name does not match its package target",
        )
        version_root = data_output / "codestory-cli" / args.expected_version
        shutil.copytree(unpacked, version_root)
        relative_cli = cli.relative_to(unpacked).as_posix()
        managed_manifest = {
            "path": relative_cli,
            "sha256": manifest["binary"]["sha256"],
            "version": args.expected_version,
            "build_source": "candidate_archive",
            "repo_ref": manifest["source"]["commit"],
            "archive": archive.name,
            "archive_url": f"candidate-archive:{sha256(archive)}",
            "archive_sha256": sha256(archive),
            "target": manifest["asset_target"],
            "stdio_initialize_verified": True,
            "provisioned_at": f"candidate-proof:{manifest['source']['commit']}",
        }
        write_json(version_root / "manifest.json", managed_manifest)
    provenance = {
        "schema_version": 1,
        "installation_source": "candidate_archive",
        "plugin_id": "codestory",
        "plugin_version": args.expected_version,
        "plugin_source_commit": manifest["source"]["commit"],
        "plugin_source_tree": manifest["source"]["tree"],
        "plugin_package_sha256": directory_contract_sha256(plugin_output),
        "candidate_archive_sha256": sha256(archive),
        "candidate_asset_target": manifest["asset_target"],
        "producer": producer,
    }
    write_json(provenance_output, provenance)
    return {
        "plugin_root": str(plugin_output),
        "plugin_data": str(data_output),
        "provenance": str(provenance_output),
        "source": manifest["source"],
        "archive_sha256": sha256(archive),
        "asset_target": manifest["asset_target"],
    }


def installed_plugin_provenance(
    args: argparse.Namespace,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    require(
        args.proof_tier == "installed_runtime",
        "installed plugin provenance is valid only at installed_runtime tier",
    )
    require(
        args.installed_plugin_provenance is not None,
        "installed_runtime proof requires --installed-plugin-provenance",
    )
    require(
        args.installed_plugin_data is not None and args.installed_plugin_data.is_dir(),
        "installed_runtime proof requires an existing --installed-plugin-data directory",
    )
    source_plugin_root = Path(__file__).resolve().parents[2] / "plugins" / "codestory"
    require(
        not os.path.samefile(plugin_root, source_plugin_root),
        "installed_runtime proof rejects the repository-source plugin root",
    )
    completed = subprocess.run(
        ["git", "-C", str(plugin_root), "rev-parse", "--show-toplevel"],
        text=True,
        capture_output=True,
        timeout=20,
    )
    if completed.returncode == 0:
        checkout = Path(completed.stdout.strip())
        require(
            not ((checkout / "Cargo.toml").is_file() and (checkout / "crates/codestory-cli").is_dir()),
            "installed_runtime proof rejects a plugin launched from a CodeStory source checkout",
        )
    try:
        provenance = json.loads(
            args.installed_plugin_provenance.read_text(encoding="utf-8")
        )
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"installed plugin provenance is not valid JSON: {exc}") from exc
    require(isinstance(provenance, dict), "installed plugin provenance must be an object")
    require(provenance.get("schema_version") == 1, "installed plugin provenance schema is unsupported")
    installation_source = args.installed_plugin_source
    if installation_source == "candidate":
        require_exact_keys(
            provenance,
            {
                "schema_version",
                "installation_source",
                "plugin_id",
                "plugin_version",
                "plugin_source_commit",
                "plugin_source_tree",
                "plugin_package_sha256",
                "candidate_archive_sha256",
                "candidate_asset_target",
                "producer",
            },
            "candidate installed plugin provenance",
        )
        require(
            provenance["installation_source"] == "candidate_archive"
            and provenance["candidate_archive_sha256"] == sha256(args.archive)
            and provenance["candidate_asset_target"] == manifest["asset_target"]
            and provenance["plugin_source_tree"] == manifest["source"]["tree"]
            and provenance["producer"]
            == {
                "repository": args.candidate_producer_repository,
                "workflow_path": args.candidate_producer_workflow_path,
                "run_id": args.candidate_producer_run_id,
                "run_attempt": args.candidate_producer_run_attempt,
                "artifact_name": args.candidate_artifact_name,
            },
            "candidate installed plugin provenance does not match the exact archive and source tree",
        )
    else:
        require(
            installation_source == "marketplace"
            and provenance.get("marketplace_repository")
            == "TheGreenCedar/AgentPluginMarketplace",
            "installed plugin provenance names the wrong marketplace",
        )
    marketplace_commit = provenance.get("marketplace_commit")
    if installation_source == "marketplace":
        require(
            isinstance(marketplace_commit, str)
            and re.fullmatch(r"[0-9a-f]{40}", marketplace_commit) is not None,
            "installed plugin provenance has an invalid marketplace commit",
        )
    require(provenance.get("plugin_id") == "codestory", "installed plugin provenance names the wrong plugin")
    require(
        provenance.get("plugin_version") == manifest["release_version"],
        "installed plugin version does not match the package",
    )
    require(
        provenance.get("plugin_source_commit") == manifest["source"]["commit"],
        "installed plugin source commit does not match the packaged source commit",
    )
    package_sha256 = directory_contract_sha256(plugin_root)
    require(
        provenance.get("plugin_package_sha256") == package_sha256,
        "installed plugin package bytes do not match their provenance",
    )
    retained = {
        "schema_version": 1,
        "installation_source": (
            "candidate_archive"
            if installation_source == "candidate"
            else "marketplace"
        ),
        "plugin_id": "codestory",
        "plugin_version": provenance["plugin_version"],
        "plugin_source_commit": provenance["plugin_source_commit"],
        "plugin_package_sha256": package_sha256,
    }
    if installation_source == "candidate":
        retained.update(
            {
                "plugin_source_tree": provenance["plugin_source_tree"],
                "candidate_archive_sha256": provenance[
                    "candidate_archive_sha256"
                ],
                "candidate_asset_target": provenance["candidate_asset_target"],
                "producer": provenance["producer"],
            }
        )
    else:
        retained.update(
            {
                "marketplace_repository": provenance["marketplace_repository"],
                "marketplace_commit": marketplace_commit,
            }
        )
    return retained


def verify_managed_runtime_status(
    status: dict,
    *,
    plugin_root: Path,
    manifest: dict,
    archive_sha256: str,
) -> dict:
    plugin = status.get("plugin_runtime")
    require(isinstance(plugin, dict), "installed status omitted plugin_runtime provenance")
    require(plugin.get("cli_source") == "managed", "installed proof did not use the managed runtime")
    require(plugin.get("local_dev_override") is False, "installed proof used a local CLI override")
    require(
        plugin.get("plugin_version") == manifest["release_version"],
        "installed plugin version does not match the package",
    )
    reported_root = plugin.get("plugin_root")
    require(isinstance(reported_root, str), "installed status omitted plugin_root")
    require(
        os.path.samefile(Path(reported_root), plugin_root),
        "installed status names a different plugin root",
    )
    require(
        plugin.get("managed_binary_sha256") == manifest["binary"]["sha256"],
        "installed managed executable does not match the package",
    )
    require(
        plugin.get("archive_sha256") == archive_sha256,
        "installed managed runtime names a different release archive",
    )
    require(
        plugin.get("cli_version") == manifest["release_version"],
        "installed managed executable version does not match the package",
    )
    managed_binary_path = plugin.get("managed_binary_path")
    require(
        isinstance(managed_binary_path, str) and Path(managed_binary_path).is_file(),
        "installed status omitted the managed executable path",
    )
    require(
        sha256(Path(managed_binary_path)) == manifest["binary"]["sha256"],
        "installed managed executable path does not contain the packaged binary",
    )
    for field in ("build_source", "repo_ref", "provisioned_at"):
        require_nonempty_string(plugin.get(field), f"installed plugin_runtime.{field}")
    return {
        "cli_source": "managed",
        "plugin_version": plugin["plugin_version"],
        "managed_binary_sha256": plugin["managed_binary_sha256"],
        "archive_sha256": plugin["archive_sha256"],
        "build_source": plugin["build_source"],
        "repo_ref": plugin["repo_ref"],
        "provisioned_at": plugin["provisioned_at"],
    }


def run_parallel(tasks: dict[str, callable]) -> dict[str, object]:
    results: dict[str, object] = {}
    failures: list[tuple[str, BaseException]] = []
    lock = threading.Lock()

    def invoke(name: str, task) -> None:
        try:
            value = task()
            with lock:
                results[name] = value
        except BaseException as exc:  # noqa: BLE001 - preserve worker failure for the proof.
            with lock:
                failures.append((name, exc))

    threads = [
        threading.Thread(target=invoke, args=(name, task), daemon=True)
        for name, task in tasks.items()
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    if failures:
        failures.sort(key=lambda item: item[0])
        details = "; ".join(f"{name}: {failure}" for name, failure in failures)
        raise ProofFailure(
            f"parallel qualification tasks failed: {details}"
        ) from failures[0][1]
    return results


def isolated_environment(root: Path, policy: str | None, offline: bool) -> dict[str, str]:
    env = dict(os.environ)
    home = root / "home"
    cache = root / "cache"
    data = root / "plugin-data"
    temp = root / "tmp"
    runtime = root / "runtime"
    for path in (home, cache, data, temp, runtime):
        path.mkdir(parents=True, exist_ok=True)
    runtime.chmod(0o700)
    env.update({
        "HOME": str(home),
        "USERPROFILE": str(home),
        "CODESTORY_CACHE_ROOT": str(cache),
        "CODESTORY_PLUGIN_DATA": str(data),
        "TMPDIR": str(temp),
        "TEMP": str(temp),
        "TMP": str(temp),
        "XDG_RUNTIME_DIR": str(runtime),
        "CODESTORY_EMBED_ALLOW_CPU": "1" if policy == "cpu_explicit" else "0",
    })
    if offline:
        env.update({
            "HTTP_PROXY": "http://127.0.0.1:1",
            "HTTPS_PROXY": "http://127.0.0.1:1",
            "ALL_PROXY": "http://127.0.0.1:1",
            "NO_PROXY": "",
            "CODESTORY_PLUGIN_DISABLE_PROVISION": "1",
        })
    for key in list(env):
        if key.startswith("CODESTORY_EMBED_") and key != "CODESTORY_EMBED_ALLOW_CPU":
            del env[key]
    return env


def qualification_environment(root: Path, env: dict[str, str]) -> tuple[dict[str, str], dict]:
    proof_root = (root / "qualification").resolve()
    proof_root.mkdir(parents=True, exist_ok=True)
    proof_root.chmod(0o700)
    nonce = secrets.token_hex(32)
    qualified = dict(env)
    qualified["CODESTORY_EMBED_QUALIFICATION_DIR"] = str(proof_root)
    qualified["CODESTORY_EMBED_QUALIFICATION_NONCE"] = nonce
    return qualified, {
        "schema_version": QUALIFICATION_SCHEMA_VERSION,
        "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
    }


def assert_no_legacy_state(cache_root: Path) -> None:
    offenders = []
    for path in cache_root.rglob("*"):
        lowered = path.name.lower()
        if any(token in lowered for token in LEGACY_TOKENS) or path.suffix.lower() == ".pid":
            offenders.append(str(path))
    require(not offenders, "legacy process state was created: " + ", ".join(offenders[:10]))


def create_second_repository(root: Path) -> Path:
    repo = root / "second-repository"
    repo.mkdir()
    (repo / "README.md").write_text("# Second repository\n\nA tiny warm-engine reuse fixture.\n", encoding="utf-8")
    (repo / "lib.rs").write_text("pub fn shared_engine_probe() -> &'static str { \"warm\" }\n", encoding="utf-8")
    return repo


def prove_runtime(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    out_dir: Path,
    manifest: dict,
) -> dict:
    require(args.plugin_handoff, "runtime proof requires the ordinary packaged plugin handoff")
    require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
    require(args.project is not None, "--project is required for runtime proof")
    project_a = args.project.resolve()
    require(project_a.is_dir(), f"first proof repository does not exist: {project_a}")
    require(
        len(args.additional_project) == len(args.additional_query),
        "each --additional-project requires one --additional-query",
    )
    if args.additional_project:
        require(
            len(args.additional_project) == 1,
            "two-host proof accepts exactly one --additional-project",
        )
        project_b = args.additional_project[0].resolve()
        query_b = args.additional_query[0]
    else:
        project_b = create_second_repository(root)
        query_b = "shared_engine_probe"
    require(project_b.is_dir(), f"second proof repository does not exist: {project_b}")
    require(project_a != project_b, "two-host proof requires different repositories")

    plugin_root = args.plugin_root.resolve()
    provenance = (
        installed_plugin_provenance(args, plugin_root, manifest)
        if args.proof_tier == "installed_runtime"
        else None
    )
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
    node = shutil.which("node")
    require(node is not None, "packaged plugin proof requires Node.js for the host launcher")
    qualified_env, qualification_control = qualification_environment(root, env)
    qualified_env.pop("CODESTORY_CLI", None)
    if args.proof_tier == "installed_runtime":
        qualified_env["CODESTORY_PLUGIN_DATA"] = str(args.installed_plugin_data.resolve())
        if args.installed_plugin_source == "candidate":
            candidate_archive_sha256 = sha256(args.archive)
            qualified_env[
                "CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256"
            ] = candidate_archive_sha256
            write_private_json(
                Path(qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"])
                / "candidate-managed-install.json",
                {
                    "schema_version": 1,
                    "purpose": "codestory-candidate-managed-install",
                    "archive_sha256": candidate_archive_sha256,
                    "qualification_nonce_sha256": hashlib.sha256(
                        qualified_env[
                            "CODESTORY_EMBED_QUALIFICATION_NONCE"
                        ].encode("ascii")
                    ).hexdigest(),
                },
            )
    else:
        qualified_env["CODESTORY_CLI"] = str(cli)
    command = [node, str(launcher)]

    embedded_models = Path(qualified_env["CODESTORY_CACHE_ROOT"]) / "embedded-models"
    require(not embedded_models.exists(), "isolated proof cache was not empty before first use")
    host_a = McpProcess(command, env=qualified_env, cwd=project_a, timeout=args.timeout_secs)
    host_b = McpProcess(command, env=qualified_env, cwd=project_b, timeout=args.timeout_secs)
    host_a_start = process_start_identity(host_a.process.pid)
    host_b_start = process_start_identity(host_b.process.pid)
    require(
        (host_a.process.pid, host_a_start) != (host_b.process.pid, host_b_start),
        "plugin hosts are not independent processes",
    )
    try:
        run_parallel({"initialize-a": host_a.initialize, "initialize-b": host_b.initialize})
        cold_started = time.perf_counter()
        cold_results = run_parallel(
            {
                "search-a": lambda: host_a.search_until_ready(
                    {"project": str(project_a), "query": args.query, "why": True},
                    "cold-search-a",
                ),
                "search-b": lambda: host_b.search_until_ready(
                    {"project": str(project_b), "query": query_b, "why": True},
                    "cold-search-b",
                ),
            }
        )
        cold_race_wall_ms = round((time.perf_counter() - cold_started) * 1000, 3)
        diagnostics_a = host_a.engine_diagnostics(project_a, "diagnostics-a")
        diagnostics_b = host_b.engine_diagnostics(project_b, "diagnostics-b")
        identity_a = engine_identity(
            diagnostics_a,
            args.engine_policy,
            args.expected_backend,
        )
        identity_b = engine_identity(
            diagnostics_b,
            args.engine_policy,
            args.expected_backend,
        )
        snapshot_a = server_snapshot(diagnostics_a, manifest, require_resident=True)
        snapshot_b = server_snapshot(diagnostics_b, manifest, require_resident=True)
        shared_identity = shared_server_identity(snapshot_a, snapshot_b)
        require(
            identity_a["embedding_engine_instance_id"]
            == identity_b["embedding_engine_instance_id"],
            "independent plugin hosts observed different engine instances",
        )
        require(
            identity_a["embedding_engine_load_generation"]
            == identity_b["embedding_engine_load_generation"]
            == shared_identity["load_generation"],
            "engine load generation disagrees with server proof",
        )
        require(
            identity_a["embedding_model_load_count"]
            == identity_b["embedding_model_load_count"]
            == shared_identity["model_load_count"]
            == 1,
            "two-host cold race did not prove one model load",
        )
        status_a = host_a.status(project_a, "status-a")
        status_b = host_b.status(project_b, "status-b")
        assert_public_status(status_a)
        assert_public_status(status_b)
        managed_runtime = None
        if args.proof_tier == "installed_runtime":
            managed_runtime = verify_managed_runtime_status(
                status_a,
                plugin_root=plugin_root,
                manifest=manifest,
                archive_sha256=sha256(args.archive),
            )
            require(
                verify_managed_runtime_status(
                    status_b,
                    plugin_root=plugin_root,
                    manifest=manifest,
                    archive_sha256=sha256(args.archive),
                )
                == managed_runtime,
                "independent installed plugin hosts reported different managed runtime provenance",
            )
            if args.installed_plugin_source == "candidate":
                require(
                    managed_runtime["build_source"] == "candidate_archive"
                    and managed_runtime["repo_ref"]
                    == manifest["source"]["commit"],
                    "candidate installed proof did not launch the staged candidate archive",
                )
            else:
                require(
                    managed_runtime["build_source"] == "github_release"
                    and managed_runtime["repo_ref"]
                    == f"v{manifest['release_version']}",
                    "marketplace installed proof did not launch the published release archive",
                )
            managed_binary_path = Path(
                require_nonempty_string(
                    status_a["plugin_runtime"].get("managed_binary_path"),
                    "installed plugin_runtime.managed_binary_path",
                )
            ).resolve()
            require(
                managed_binary_path.is_relative_to(args.installed_plugin_data.resolve()),
                "installed managed executable is outside the installed plugin data root",
            )
            require(
                managed_binary_path != cli.resolve(),
                "installed proof used the unpacked package executable as its managed runtime",
            )

        before_encode = snapshot_a["engine"]["successful_encode_count"]
        run_parallel(
            {
                "packet-a": lambda: host_a.tool_until_ready(
                    "packet",
                    {
                        "project": str(project_a),
                        "question": args.question,
                        "budget": "compact",
                    },
                    "packet-a",
                ),
                "search-b-live": lambda: host_b.search_until_ready(
                    {"project": str(project_b), "query": query_b, "why": True},
                    "search-b-live",
                ),
            }
        )
        after_diagnostics = host_b.engine_diagnostics(project_b, "diagnostics-after-live")
        after_snapshot = server_snapshot(after_diagnostics, manifest, require_resident=True)
        require(
            after_snapshot["engine"]["successful_encode_count"] > before_encode,
            "successful encode counter did not advance across two-host retrieval",
        )
        require(
            after_snapshot["process"]["server_instance_id"]
            == shared_identity["server_instance_id"],
            "live retrieval replaced the shared server",
        )
        memory_observations = (
            capture_five_process_memory(
                args=args,
                node_path=Path(node),
                host_a=host_a,
                host_a_start=host_a_start,
                host_b=host_b,
                host_b_start=host_b_start,
                status_a=status_a,
                status_b=status_b,
                snapshot=after_snapshot,
                manifest=manifest,
                expected_backend=identity_a["embedding_backend"],
            )
            if args.produce_qualification_evidence
            else None
        )

        host_a.kill()
        host_b.search_until_ready(
            {"project": str(project_b), "query": query_b, "why": True},
            "survivor-search",
        )
        survivor = server_snapshot(
            host_b.engine_diagnostics(project_b, "survivor-diagnostics"),
            manifest,
            require_resident=True,
        )
        require(
            survivor["process"]["server_instance_id"] == shared_identity["server_instance_id"],
            "one client exit disrupted the surviving client or replaced the server",
        )

        host_c = McpProcess(command, env=qualified_env, cwd=project_a, timeout=args.timeout_secs)
        host_c_start = process_start_identity(host_c.process.pid)
        try:
            require(
                (host_c.process.pid, host_c_start)
                not in {
                    (host_a.process.pid, host_a_start),
                    (host_b.process.pid, host_b_start),
                },
                "replacement plugin host was not independently started",
            )
            host_c.initialize()
            host_c.search_until_ready(
                {"project": str(project_a), "query": args.query, "why": True},
                "rejoin-search",
            )
            rejoin_diagnostics = host_c.engine_diagnostics(project_a, "rejoin-diagnostics")
            rejoin_identity = engine_identity(
                rejoin_diagnostics,
                args.engine_policy,
                args.expected_backend,
            )
            rejoin_snapshot = server_snapshot(
                rejoin_diagnostics,
                manifest,
                require_resident=True,
            )
            require(
                rejoin_snapshot["process"]["server_instance_id"]
                == shared_identity["server_instance_id"],
                "new plugin host did not join the existing server",
            )
        finally:
            write_json(
                out_dir / "plugin-host-c-mcp.json",
                retained_mcp_transcript(host_c.transcript),
            )
            host_c.close()

        cold_models = list(embedded_models.rglob("*.gguf"))
        require(len(cold_models) == 1, "two-host first use did not materialize exactly one model")
        materialized = cold_models[0]
        require(
            sha256(materialized) == identity_a["embedding_model_sha256"],
            "materialized model digest does not match runtime identity",
        )
        result = {
            "proof_tier": args.proof_tier,
            "qualification_control": qualification_control,
            "same_account": {
                "account_id": current_account_identity(),
                "relation": "same_os_account",
                "cross_login_or_terminal_sessions_proven": False,
                "plugin_hosts": [
                    {
                        "pid": host_a.process.pid,
                        "process_start_id": host_a_start,
                        "repository_id": opaque_repository_id(project_a),
                    },
                    {
                        "pid": host_b.process.pid,
                        "process_start_id": host_b_start,
                        "repository_id": opaque_repository_id(project_b),
                    },
                ],
            },
            "cold_race_wall_ms": cold_race_wall_ms,
            "cold_search_attempts": {
                "host_a": cold_results["search-a"][1],
                "host_b": cold_results["search-b"][1],
            },
            "shared_identity": shared_identity,
            "snapshot_a": snapshot_a,
            "snapshot_b": snapshot_b,
            "survivor_snapshot": survivor,
            "rejoin_snapshot": rejoin_snapshot,
            "identity": identity_a,
            "second_host_identity": identity_b,
            "rejoin_identity": rejoin_identity,
            "materialization": {
                "sha256": sha256(materialized),
                "reused_on_rejoin": rejoin_identity["embedding_materialized_reused"],
            },
            "installed_plugin": provenance,
            "managed_runtime": managed_runtime,
            "_qualification_cli_path": (
                str(managed_binary_path)
                if args.proof_tier == "installed_runtime"
                else str(cli.resolve())
            ),
            "_qualification_projects": [str(project_a), str(project_b)],
            "_memory_observations": memory_observations,
            "_qualification_forbidden_values": [
                str(project_a),
                str(project_b),
                str(plugin_root),
                str(cli.resolve()),
                str(root.resolve()),
                qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"],
                qualified_env["CODESTORY_EMBED_QUALIFICATION_NONCE"],
                args.query,
                args.question,
                query_b,
                *(
                    [str(managed_binary_path)]
                    if args.proof_tier == "installed_runtime"
                    else []
                ),
            ],
            "nonclaims": {
                claim: {
                    "claimed": False,
                    "reason": "hosted two-process package evidence does not establish this claim",
                }
                for claim in sorted(LOWER_TIER_NONCLAIMS)
            },
        }
    finally:
        write_json(
            out_dir / "plugin-host-a-mcp.json",
            retained_mcp_transcript(host_a.transcript),
        )
        write_json(
            out_dir / "plugin-host-b-mcp.json",
            retained_mcp_transcript(host_b.transcript),
        )
        host_a.close()
        host_b.close()
    assert_no_legacy_state(Path(qualified_env["CODESTORY_CACHE_ROOT"]))
    public_runtime_evidence = out_dir / "two-host-server-proof.json"
    write_json(public_runtime_evidence, retained_runtime_evidence(result))
    forbidden_runtime_values = result.get("_qualification_forbidden_values", [])
    for public_artifact in (
        out_dir / "plugin-host-a-mcp.json",
        out_dir / "plugin-host-b-mcp.json",
        out_dir / "plugin-host-c-mcp.json",
        public_runtime_evidence,
    ):
        assert_retained_json_privacy(public_artifact, forbidden_runtime_values)
    return result


def metric_passes(value: int | float, threshold: int | float, comparison: str) -> bool:
    return {
        "equal": value == threshold,
        "greater_than_or_equal": value >= threshold,
        "less_than_or_equal": value <= threshold,
    }[comparison]


def write_private_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(value, handle, sort_keys=True, separators=(",", ":"))
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        path.chmod(0o600)
    except BaseException:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def read_jsonl(path: Path) -> list[dict]:
    if not path.is_file() or path.is_symlink():
        return []
    events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def wait_for_jsonl_event(
    path: Path,
    predicate,
    *,
    timeout: int,
    process: subprocess.Popen | None = None,
) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for event in read_jsonl(path):
            if predicate(event):
                return event
        if process is not None and process.poll() is not None:
            stdout, stderr = process.communicate()
            raise ProofFailure(
                "qualification product process exited before its raw event: "
                f"exit={process.returncode} stdout_sha256="
                f"{hashlib.sha256(stdout.encode('utf-8')).hexdigest()} stderr_sha256="
                f"{hashlib.sha256(stderr.encode('utf-8')).hexdigest()}"
            )
        time.sleep(0.01)
    raise ProofFailure(f"timed out waiting for qualification event file {path.name}")


def send_server_qualification_control(
    directory: Path,
    nonce: str,
    *,
    sequence: int,
    action: str,
    timeout: int,
) -> dict:
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    command_path = directory / f"{nonce}.command.json"
    require(not command_path.exists(), "stale embedding qualification command is present")
    write_private_json(
        command_path,
        {
            "schema_version": 1,
            "sequence": sequence,
            "nonce_sha256": nonce_sha256,
            "action": action,
            "parameters": {"class": None},
        },
    )
    try:
        event_path = directory / f"{nonce}.events.jsonl"
        event = wait_for_jsonl_event(
            event_path,
            lambda candidate: candidate.get("sequence") == sequence
            and candidate.get("action") == action,
            timeout=timeout,
        )
        require(
            event.get("status") in {"completed", "accepted"},
            f"embedding qualification control {action} failed",
        )
        return event
    finally:
        command_path.unlink(missing_ok=True)


def server_observation_from_control_event(event: dict, phase: str) -> dict:
    snapshot = event.get("snapshot")
    require(isinstance(snapshot, dict), f"{phase} control event omitted its server snapshot")
    process = snapshot.get("process")
    engine = snapshot.get("engine")
    require(isinstance(process, dict), f"{phase} server snapshot omitted process identity")
    require(isinstance(engine, dict), f"{phase} server snapshot omitted resident engine identity")
    process_start = require_nonempty_string(
        process.get("process_start_id"), f"{phase} process start"
    )
    return {
        "phase": phase,
        "server_instance_id": require_opaque_identifier(
            process.get("server_instance_id"), f"{phase} server instance"
        ),
        "process_start_id": hashlib.sha256(process_start.encode("utf-8")).hexdigest(),
        "load_generation": require_positive_int(
            engine.get("load_generation"), f"{phase} load generation"
        ),
    }


def publication_identity_from_status(status: dict) -> str:
    require(status.get("retrieval_mode") == "full", "qualification status is not full")
    contract = status.get("manifest_contract")
    manifest = status.get("manifest")
    require(
        isinstance(contract, dict),
        "qualification status omitted its manifest contract",
    )
    require(
        isinstance(manifest, dict),
        "qualification status omitted its published manifest",
    )
    generation = require_nonempty_string(
        contract.get("generation"), "qualification manifest contract generation"
    )
    input_hash = require_sha256(
        contract.get("input_hash"), "qualification manifest contract input hash"
    )
    project_id = require_opaque_identifier(
        contract.get("project_id"), "qualification manifest contract project"
    )
    schema_version = require_positive_int(
        contract.get("schema_version"), "qualification manifest contract schema"
    )
    graph_hash = require_sha256(
        contract.get("graph_hash"), "qualification manifest contract graph hash"
    )
    require(
        manifest.get("project_id") == project_id
        and manifest.get("sidecar_generation") == generation
        and manifest.get("sidecar_input_hash") == input_hash
        and manifest.get("sidecar_schema_version") == schema_version
        and manifest.get("graph_artifact_hash") == graph_hash,
        "qualification manifest report disagrees with its manifest contract",
    )
    return canonical_sha256(
        {
            "project_id": project_id,
            "generation": generation,
            "input_hash": input_hash,
            "schema_version": schema_version,
            "graph_hash": graph_hash,
            "lexical_version": require_nonempty_string(
                manifest.get("lexical_version"),
                "qualification manifest lexical version",
            ),
            "semantic_generation": require_nonempty_string(
                manifest.get("semantic_generation"),
                "qualification manifest semantic generation",
            ),
            "scip_revision": require_nonempty_string(
                manifest.get("scip_revision"),
                "qualification manifest SCIP revision",
            ),
        }
    )


def run_quality_search(
    cli: Path,
    env: dict[str, str],
    project: Path,
    run_id: str,
    query: str,
    expected: str,
    *,
    timeout: int,
) -> tuple[int | None, str]:
    result, payload = json_command(
        [
            str(cli),
            "search",
            "--project",
            str(project),
            "--query",
            query,
            "--limit",
            "10",
            "--repo-text",
            "off",
            "--refresh",
            "none",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    hits = payload.get("indexed_symbol_hits")
    require(isinstance(hits, list), "qualification search omitted indexed symbol hits")
    position = next(
        (
            index
            for index, hit in enumerate(hits)
            if isinstance(hit, dict)
            and isinstance(hit.get("display_name"), str)
            and expected in hit["display_name"]
        ),
        None,
    )
    rank = None if position is None or position >= 10 else position + 1
    output_sha256 = hashlib.sha256(result["stdout"].encode("utf-8")).hexdigest()
    return rank, output_sha256


def run_publication_replacement_worker(
    cli: Path,
    env: dict[str, str],
    project: Path,
    private_root: Path,
    nonce: str,
    *,
    timeout: int,
) -> None:
    request_path = private_root / "publication-replacement-worker-request.json"
    output_path = private_root / "publication-replacement-worker-output.json"
    write_private_json(
        request_path,
        {
            "schema_version": 1,
            "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
            "executable_sha256": sha256(cli),
            "project": str(project.resolve()),
            "operation": "query",
            "parameters": {
                "query_count": 1,
                "bulk_count": 0,
                "documents_per_bulk": 0,
                "input_bytes": 64,
                "hold_ms": 0,
            },
        },
    )
    run(
        [
            str(cli),
            "internal-embedding-qualification-worker",
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    require(
        output_path.is_file() and not output_path.is_symlink(),
        "publication replacement worker omitted its output",
    )
    try:
        output = json.loads(output_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"publication replacement worker output is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(output, dict)
        and output.get("schema_version") == 1
        and output.get("executable_sha256") == sha256(cli)
        and output.get("error") is None,
        "publication replacement worker failed",
    )
    result = output.get("result")
    operations = result.get("operations") if isinstance(result, dict) else None
    require(
        isinstance(result, dict)
        and result.get("schema_version") == 1
        and result.get("scenario") == "query"
        and isinstance(operations, list)
        and len(operations) == 1
        and operations[0].get("status") == "ok"
        and operations[0].get("error_code") is None,
        "publication replacement worker did not complete its query",
    )


def produce_product_publication_fault_evidence(
    cli: Path,
    env: dict[str, str],
    private_root: Path,
    artifact_root: Path,
    nonce: str,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    timeout: int,
) -> tuple[Path, Path]:
    project = private_root / "publication-product-repository"
    project.mkdir(mode=0o700)
    anchors = [
        f"qualification_anchor_{index:02d}"
        for index in range(FAULT_RECOVERY_CONSISTENCY_CASES)
    ]
    source_file = project / "lib.rs"
    baseline_source = (
        "\n".join(
            f'pub fn {anchor}() -> &\'static str {{ "{anchor}" }}' for anchor in anchors
        )
        + "\n"
    )
    source_file.write_text(baseline_source, encoding="utf-8")
    lexical_file = project / "README.md"
    baseline_lexical = "# Publication qualification baseline\n"
    lexical_file.write_text(baseline_lexical, encoding="utf-8")
    baseline_file_times = {
        path: (metadata.st_atime_ns, metadata.st_mtime_ns)
        for path in (source_file, lexical_file)
        for metadata in (path.stat(),)
    }
    run_id = "publication-qualification"
    index_command = [
        str(cli),
        "index",
        "--project",
        str(project),
        "--refresh",
        "full",
        "--format",
        "json",
    ]
    retrieval_index_command = [
        str(cli),
        "retrieval",
        "index",
        "--project",
        str(project),
        "--profile",
        "agent",
        "--run-id",
        run_id,
        "--refresh",
        "none",
        "--format",
        "json",
    ]
    status_command = [
        str(cli),
        "retrieval",
        "status",
        "--project",
        str(project),
        "--profile",
        "agent",
        "--run-id",
        run_id,
        "--format",
        "json",
    ]
    json_command(index_command, env=env, cwd=project, timeout=timeout)
    json_command(retrieval_index_command, env=env, cwd=project, timeout=timeout)
    baseline_status_result, baseline_status = json_command(
        status_command, env=env, cwd=project, timeout=timeout
    )
    previous_publication = publication_identity_from_status(baseline_status)
    baseline_ranks = []
    for anchor in anchors:
        rank, _ = run_quality_search(
            cli, env, project, run_id, anchor, anchor, timeout=timeout
        )
        baseline_ranks.append(rank)

    snapshot_before = send_server_qualification_control(
        private_root, nonce, sequence=1, action="snapshot", timeout=timeout
    )
    correlation_id = secrets.token_hex(16)
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    pause_path = private_root / f"publication-pause-{nonce_sha256}.json"
    resume_path = private_root / f"publication-resume-{correlation_id}.json"
    hook_event_path = private_root / f"publication-events-{correlation_id}.jsonl"
    write_private_json(
        pause_path,
        {
            "schema_version": 1,
            "nonce_sha256": nonce_sha256,
            "correlation_id": correlation_id,
            "action": "pause_before_manifest_commit",
        },
    )
    source_file.write_text(
        baseline_source + "// publication qualification candidate source change\n",
        encoding="utf-8",
    )
    lexical_file.write_text("# Publication qualification candidate\n", encoding="utf-8")
    candidate = subprocess.Popen(
        retrieval_index_command,
        cwd=project,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    candidate_stdout = ""
    candidate_stderr = ""
    try:
        wait_for_jsonl_event(
            hook_event_path,
            lambda event: event.get("action") == "pause_before_manifest_commit"
            and event.get("status") == "waiting_for_resume",
            timeout=timeout,
            process=candidate,
        )
        send_server_qualification_control(
            private_root, nonce, sequence=2, action="crash_server", timeout=timeout
        )
        run_publication_replacement_worker(
            cli,
            env,
            project,
            private_root,
            nonce,
            timeout=timeout,
        )
        snapshot_after = send_server_qualification_control(
            private_root, nonce, sequence=3, action="snapshot", timeout=timeout
        )
        write_private_json(
            resume_path,
            {
                "schema_version": 1,
                "nonce_sha256": nonce_sha256,
                "correlation_id": correlation_id,
                "action": "resume_manifest_commit",
            },
        )
        candidate_stdout, candidate_stderr = candidate.communicate(timeout=timeout)
    except BaseException:
        if candidate.poll() is None:
            candidate.kill()
            candidate_stdout, candidate_stderr = candidate.communicate()
        raise
    finally:
        source_file.write_text(baseline_source, encoding="utf-8")
        lexical_file.write_text(baseline_lexical, encoding="utf-8")
        for path, times in baseline_file_times.items():
            os.utime(path, ns=times)
    require(
        candidate.returncode is not None and candidate.returncode != 0,
        "publication candidate did not fail after losing its server lease",
    )
    hook_events = read_jsonl(hook_event_path)
    require(len(hook_events) == 4, "publication hook did not emit its exact four events")
    final_status_result, final_status = json_command(
        status_command, env=env, cwd=project, timeout=timeout
    )
    final_publication = publication_identity_from_status(final_status)
    post_ranks = []
    post_search_sha256 = None
    for anchor in anchors:
        rank, output_sha256 = run_quality_search(
            cli, env, project, run_id, anchor, anchor, timeout=timeout
        )
        post_ranks.append(rank)
        if post_search_sha256 is None:
            post_search_sha256 = output_sha256
    require(post_search_sha256 is not None, "qualification search emitted no output digest")

    publication_payload = {
        "schema_version": 1,
        "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "correlation_id": correlation_id,
        "previous_publication_identity_sha256": previous_publication,
        "server_observations": [
            server_observation_from_control_event(snapshot_before, "before_crash"),
            server_observation_from_control_event(snapshot_after, "after_replacement"),
        ],
        "candidate_observation": {
            "command": "retrieval_index",
            "exit_code": candidate.returncode,
            "stdout_sha256": hashlib.sha256(candidate_stdout.encode("utf-8")).hexdigest(),
            "stderr_sha256": hashlib.sha256(candidate_stderr.encode("utf-8")).hexdigest(),
        },
        "publication_hook_events": hook_events,
        "ordinary_product_observations": [
            {
                "sequence": 0,
                "command": "retrieval_status",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": hashlib.sha256(
                    final_status_result["stdout"].encode("utf-8")
                ).hexdigest(),
            },
            {
                "sequence": 1,
                "command": "search",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": post_search_sha256,
            },
        ],
    }
    publication_path = artifact_root / "publication-fault-external.raw.json"
    write_private_json(publication_path, publication_payload)
    consistency_payload = {
        "schema_version": 1,
        "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "run_id_sha256": hashlib.sha256(correlation_id.encode("ascii")).hexdigest(),
        "observations": [
            {
                "case_id_sha256": hashlib.sha256(anchor.encode("utf-8")).hexdigest(),
                "before_server_fault_rank": baseline_ranks[index],
                "after_server_replacement_rank": post_ranks[index],
            }
            for index, anchor in enumerate(anchors)
        ],
    }
    consistency_path = artifact_root / "fault-recovery-consistency.raw.json"
    write_private_json(consistency_path, consistency_payload)
    for path in (pause_path, resume_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    return publication_path, consistency_path


def load_external_raw_evidence(path: Path, label: str) -> tuple[dict, str]:
    require(path.is_file() and not path.is_symlink(), f"{label} is missing or unsafe: {path}")
    metadata = path.stat()
    require(stat.S_ISREG(metadata.st_mode), f"{label} is not a regular file")
    require(metadata.st_size <= 8 * 1024 * 1024, f"{label} exceeds the 8 MiB evidence limit")
    payload_bytes = path.read_bytes()
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(payload, dict), f"{label} must be an object")
    return payload, hashlib.sha256(payload_bytes).hexdigest()


def require_opaque_identifier(value: object, field: str, *, length: int = 128) -> str:
    require(
        isinstance(value, str)
        and 1 <= len(value) <= length
        and re.fullmatch(r"[A-Za-z0-9._:-]+", value) is not None,
        f"{field} must be an opaque identifier without path or request text",
    )
    return value


def verify_publication_fault_raw_evidence(
    path: Path,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path, "publication fault raw evidence"
    )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "evidence_contract",
            "source",
            "package",
            "contracts",
            "correlation_id",
            "previous_publication_identity_sha256",
            "server_observations",
            "candidate_observation",
            "publication_hook_events",
            "ordinary_product_observations",
        },
        "publication fault raw evidence",
    )
    require(payload["schema_version"] == 1, "publication fault evidence schema is unsupported")
    require(
        payload["evidence_contract"] == PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "publication fault evidence contract is unsupported",
    )
    require(payload["source"] == source, "publication fault evidence source identity is stale")
    require(payload["package"] == package, "publication fault evidence package identity is stale")
    require(payload["contracts"] == contracts, "publication fault evidence contracts are stale")
    correlation_id = payload["correlation_id"]
    require(
        isinstance(correlation_id, str)
        and re.fullmatch(r"[0-9a-f]{32}", correlation_id) is not None,
        "publication fault correlation id is invalid",
    )
    previous_publication = require_sha256(
        payload["previous_publication_identity_sha256"],
        "publication fault previous publication identity",
    )

    server_observations = payload["server_observations"]
    require(
        isinstance(server_observations, list) and len(server_observations) == 2,
        "publication fault evidence requires before-crash and after-replacement server observations",
    )
    expected_server_phases = ("before_crash", "after_replacement")
    for index, (observation, phase) in enumerate(
        zip(server_observations, expected_server_phases)
    ):
        require(isinstance(observation, dict), f"server observation {index} is malformed")
        require_exact_keys(
            observation,
            {"phase", "server_instance_id", "process_start_id", "load_generation"},
            f"server observation {index}",
        )
        require(observation["phase"] == phase, f"server observation {index} has the wrong phase")
        require_opaque_identifier(
            observation["server_instance_id"],
            f"server observation {index}.server_instance_id",
        )
        require_opaque_identifier(
            observation["process_start_id"],
            f"server observation {index}.process_start_id",
        )
        require_positive_int(
            observation["load_generation"],
            f"server observation {index}.load_generation",
        )
    require(
        (
            server_observations[0]["server_instance_id"],
            server_observations[0]["process_start_id"],
        )
        != (
            server_observations[1]["server_instance_id"],
            server_observations[1]["process_start_id"],
        ),
        "publication fault evidence did not observe a replacement server",
    )

    candidate = payload["candidate_observation"]
    require(isinstance(candidate, dict), "publication candidate observation is malformed")
    require_exact_keys(
        candidate,
        {"command", "exit_code", "stdout_sha256", "stderr_sha256"},
        "publication candidate observation",
    )
    require(candidate["command"] == "retrieval_index", "publication candidate used the wrong command")
    require(
        isinstance(candidate["exit_code"], int)
        and not isinstance(candidate["exit_code"], bool)
        and candidate["exit_code"] != 0,
        "publication candidate unexpectedly committed successfully",
    )
    require_sha256(candidate["stdout_sha256"], "publication candidate stdout sha256")
    require_sha256(candidate["stderr_sha256"], "publication candidate stderr sha256")

    events = payload["publication_hook_events"]
    expected_events = (
        ("pause_before_manifest_commit", "waiting_for_resume"),
        ("resume_manifest_commit", "observed"),
        ("lease_revalidation", "failed"),
        ("manifest_commit", "returned_error"),
    )
    require(
        isinstance(events, list) and len(events) == len(expected_events),
        "publication hook evidence must contain the exact four raw fence events",
    )
    last_elapsed = -1
    for index, (event, expected) in enumerate(zip(events, expected_events)):
        require(isinstance(event, dict), f"publication hook event {index} is malformed")
        require_exact_keys(
            event,
            {"schema_version", "sequence", "correlation_id", "action", "status", "clock"},
            f"publication hook event {index}",
        )
        require(event["schema_version"] == 1, f"publication hook event {index} schema is unsupported")
        require(event["sequence"] == index, "publication hook event sequence is not exact")
        require(event["correlation_id"] == correlation_id, "publication hook correlation changed")
        require(
            (event["action"], event["status"]) == expected,
            f"publication hook event {index} does not match the fence contract",
        )
        clock = event["clock"]
        require(isinstance(clock, dict), f"publication hook event {index} omitted its clock")
        require_exact_keys(clock, {"domain", "api", "elapsed_ns"}, f"publication hook event {index} clock")
        require(
            clock["domain"] == "process_monotonic"
            and clock["api"] == "std::time::Instant",
            "publication hook used an unsupported clock",
        )
        elapsed = require_nonnegative_int(
            clock["elapsed_ns"], f"publication hook event {index} elapsed_ns"
        )
        require(elapsed >= last_elapsed, "publication hook elapsed time moved backwards")
        last_elapsed = elapsed

    ordinary = payload["ordinary_product_observations"]
    require(
        isinstance(ordinary, list) and len(ordinary) == 2,
        "publication fault evidence requires status and query product observations",
    )
    for index, (observation, command) in enumerate(
        zip(ordinary, ("retrieval_status", "search"))
    ):
        require(isinstance(observation, dict), f"ordinary product observation {index} is malformed")
        require_exact_keys(
            observation,
            {
                "sequence",
                "command",
                "exit_code",
                "retrieval_mode",
                "publication_identity_sha256",
                "output_sha256",
            },
            f"ordinary product observation {index}",
        )
        require(observation["sequence"] == index, "ordinary product observation order changed")
        require(observation["command"] == command, "ordinary product observation used the wrong command")
        require(observation["exit_code"] == 0, f"ordinary product {command} failed")
        require(observation["retrieval_mode"] == "full", f"ordinary product {command} was not full")
        require(
            require_sha256(
                observation["publication_identity_sha256"],
                f"ordinary product {command} publication identity",
            )
            == previous_publication,
            f"ordinary product {command} did not use the previous publication",
        )
        require_sha256(observation["output_sha256"], f"ordinary product {command} output sha256")

    return {
        "artifact": {
            "name": "publication-fault-external.raw.json",
            "sha256": artifact_sha256,
        },
        "assertions": {
            "lost_publication_lease_blocks_commit": True,
            "previous_publication_remains_usable": True,
        },
    }


def verify_fault_recovery_consistency_raw_evidence(
    path: Path,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path, "fault recovery consistency raw evidence"
    )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "evidence_contract",
            "source",
            "package",
            "contracts",
            "run_id_sha256",
            "observations",
        },
        "fault recovery consistency raw evidence",
    )
    require(
        payload["schema_version"] == 1,
        "fault recovery consistency evidence schema is unsupported",
    )
    require(
        payload["evidence_contract"] == FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "fault recovery consistency evidence contract is unsupported",
    )
    require(
        payload["source"] == source,
        "fault recovery consistency source identity is stale",
    )
    require(
        payload["package"] == package,
        "fault recovery consistency package identity is stale",
    )
    require(
        payload["contracts"] == contracts,
        "fault recovery consistency contracts are stale",
    )
    require_sha256(payload["run_id_sha256"], "fault recovery consistency run id")
    observations = payload["observations"]
    require(
        isinstance(observations, list)
        and len(observations) == FAULT_RECOVERY_CONSISTENCY_CASES,
        "fault recovery consistency evidence has the wrong case count",
    )
    case_ids: set[str] = set()
    for index, observation in enumerate(observations):
        require(
            isinstance(observation, dict),
            f"fault recovery consistency observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            {
                "case_id_sha256",
                "before_server_fault_rank",
                "after_server_replacement_rank",
            },
            f"fault recovery consistency observation {index}",
        )
        case_id = require_sha256(
            observation["case_id_sha256"],
            f"fault recovery consistency observation {index} case id",
        )
        require(
            case_id not in case_ids,
            "fault recovery consistency evidence contains duplicate cases",
        )
        case_ids.add(case_id)
        for field in ("before_server_fault_rank", "after_server_replacement_rank"):
            rank = observation[field]
            require(
                rank is None
                or (
                    isinstance(rank, int)
                    and not isinstance(rank, bool)
                    and 1 <= rank <= 10
                ),
                f"fault recovery consistency observation {index} {field} is not a rank in the fixed top 10",
            )
        require(
            observation["before_server_fault_rank"]
            == observation["after_server_replacement_rank"],
            "fault recovery changed a search rank from the retained publication",
        )
    return {
        "artifact": {
            "name": "fault-recovery-consistency.raw.json",
            "sha256": artifact_sha256,
        },
        "case_count": len(observations),
        "ranks_stable": True,
    }


def verify_retrieval_quality_raw_evidence(
    path: Path,
    *,
    source: dict,
    holdout_task_root: Path = HOLDOUT_TASK_ROOT,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path, "publishable packet quality raw evidence"
    )
    release_evidence = payload.get("release_evidence")
    require(
        isinstance(release_evidence, dict),
        "publishable packet evidence omitted release_evidence",
    )
    for field in ("assertions", "accepted", "decision"):
        require(
            field not in payload and field not in release_evidence,
            f"publishable packet evidence contains self-declared {field}",
        )
    require(
        release_evidence.get("commit") == source["commit"],
        "publishable packet evidence source commit is stale",
    )
    require(
        release_evidence.get("source_tree") == source["tree"],
        "publishable packet evidence source tree is stale",
    )
    require(
        release_evidence.get("evaluation_contract")
        == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "publishable packet evaluation contract is unsupported",
    )
    holdout_tasks, holdout_manifest_set_sha256 = load_holdout_task_contracts(
        holdout_task_root
    )
    evidence_identity = release_evidence.get("evidence_identity")
    require(
        isinstance(evidence_identity, dict)
        and evidence_identity.get("corpus_id") == RELEASE_QUALITY_CORPUS_ID,
        "publishable packet evidence is not bound to the release holdout corpus",
    )
    repeats = require_positive_int(
        release_evidence.get("repeats"),
        "publishable packet repeat count",
    )
    require(
        repeats == MIN_RETRIEVAL_QUALITY_REPEATS,
        f"publishable packet evidence requires exactly {MIN_RETRIEVAL_QUALITY_REPEATS} repeats",
    )
    require(
        release_evidence.get("publishable") is True,
        "packet quality artifact is not publishable",
    )
    require(
        release_evidence.get("quality_gate_status") == "pass",
        "packet quality artifact did not pass its quality gate",
    )
    blockers = release_evidence.get("publishable_blockers")
    require(
        isinstance(blockers, list) and not blockers,
        "packet quality artifact contains publishable blockers",
    )
    require(
        payload.get("repeats") == repeats,
        "packet quality top-level repeat count changed",
    )
    modes = payload.get("modes")
    require(
        isinstance(modes, list) and modes == list(RELEASE_QUALITY_MODES),
        "packet quality artifact must contain only the release cold-cli mode",
    )
    expected_modes = set(RELEASE_QUALITY_MODES.values())
    expected_cells = {
        (repo, task_id, mode, repeat)
        for repo, task_id in holdout_tasks
        for mode in expected_modes
        for repeat in range(1, MIN_RETRIEVAL_QUALITY_REPEATS + 1)
    }

    rows = release_evidence.get("rows")
    require(
        isinstance(rows, list) and rows,
        "packet quality artifact has no quality rows",
    )
    observed_cells: set[tuple[str, str, str, int]] = set()
    passing_rows = 0
    for index, row in enumerate(rows):
        require(isinstance(row, dict), f"packet quality row {index} is malformed")
        quality = row.get("quality")
        sufficiency = row.get("sufficiency")
        latency = row.get("packet_latency")
        require(
            row.get("status") == "pass"
            and isinstance(quality, dict)
            and quality.get("pass") is True,
            f"packet quality row {index} did not pass",
        )
        require(
            isinstance(sufficiency, dict)
            and sufficiency.get("status") == "sufficient"
            and sufficiency.get("sufficient_quality_mismatch") is not True,
            f"packet quality row {index} is not sufficient",
        )
        for field in (
            "follow_up_commands_count",
            "open_next_count",
            "gaps_count",
            "coverage_unresolved_blocking_count",
        ):
            value = sufficiency.get(field, 0)
            require(
                isinstance(value, (int, float))
                and not isinstance(value, bool)
                and value == 0,
                f"packet quality row {index} has unresolved {field}",
            )
        require(
            isinstance(latency, dict)
            and latency.get("sla_missed") is False
            and isinstance(latency.get("retrieval_shadow"), dict)
            and latency["retrieval_shadow"].get("retrieval_mode") == "full",
            f"packet quality row {index} lacks full-retrieval latency proof",
        )

        provenance = row.get("repo_provenance")
        require(
            isinstance(provenance, dict)
            and provenance.get("manifest_overridden_by_builtin") is False
            and provenance.get("git_dirty") is False,
            f"packet quality row {index} has untrusted repository provenance",
        )
        configured = provenance.get("configured")
        manifest_repo = provenance.get("manifest")
        require(
            isinstance(configured, dict) and isinstance(manifest_repo, dict),
            f"packet quality row {index} omitted repository identities",
        )
        configured_ref = configured.get("ref")
        require(
            isinstance(configured_ref, str)
            and re.fullmatch(r"[0-9a-f]{40}", configured_ref) is not None
            and manifest_repo.get("ref") == configured_ref
            and provenance.get("git_head") == configured_ref,
            f"packet quality row {index} is not pinned to one immutable repository commit",
        )
        trusted_repo_url = re.compile(
            r"^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?$"
        )
        urls = (
            configured.get("url"),
            manifest_repo.get("url"),
            provenance.get("git_origin"),
        )
        require(
            all(
                isinstance(url, str) and trusted_repo_url.fullmatch(url) is not None
                for url in urls
            ),
            f"packet quality row {index} has an untrusted repository URL",
        )
        normalized_urls = {
            re.sub(r"\.git$", "", url, flags=re.IGNORECASE).lower()
            for url in urls
        }
        require(
            len(normalized_urls) == 1,
            f"packet quality row {index} repository URLs disagree",
        )

        cache = row.get("codestory_cache_provenance")
        require(
            isinstance(cache, dict)
            and cache.get("doctor_status") == "pass"
            and bool(cache.get("storage_path"))
            and bool(cache.get("cache_policy"))
            and cache.get("cache_policy") != "unprepared-cache-blocked"
            and cache.get("retrieval_mode") == "full"
            and bool(cache.get("semantic_generation"))
            and bool(cache.get("manifest_embedding_backend"))
            and bool(cache.get("embedding_engine_instance_id"))
            and cache.get("embedding_policy") in {"accelerated", "cpu_explicit"}
            and cache.get("semantic_backend") is not None
            and cache.get("local_only") is True
            and cache.get("indexed") is True
            and cache.get("freshness_status") == "fresh"
            and cache.get("semantic_ready") is True
            and cache.get("indexing_in_timed_run") is not None,
            f"packet quality row {index} has incomplete CodeStory cache provenance",
        )

        repeat = row.get("repeat")
        require(
            isinstance(repeat, int)
            and not isinstance(repeat, bool)
            and 1 <= repeat <= repeats,
            f"packet quality row {index} has an invalid repeat",
        )
        repo = require_nonempty_string(row.get("repo"), f"packet quality row {index} repo")
        task_id = require_nonempty_string(
            row.get("task_id"), f"packet quality row {index} task id"
        )
        mode = require_nonempty_string(
            row.get("mode"), f"packet quality row {index} mode"
        )
        require(
            mode in expected_modes,
            f"packet quality row {index} mode is not declared at top level",
        )
        task_contract = holdout_tasks.get((repo, task_id))
        require(
            task_contract is not None,
            f"packet quality row {index} is not one of the checked-in holdout tasks",
        )
        snapshot = row.get("task_manifest_snapshot")
        require(
            isinstance(snapshot, dict),
            f"packet quality row {index} omitted its task manifest snapshot",
        )
        snapshot_without_path = {
            key: value for key, value in snapshot.items() if key != "manifest_path"
        }
        require(
            snapshot_without_path == task_contract["snapshot"],
            f"packet quality row {index} task snapshot differs from the checked-in manifest",
        )
        manifest_path = snapshot.get("manifest_path")
        require(
            isinstance(manifest_path, str)
            and Path(manifest_path).name == task_contract["path"].name,
            f"packet quality row {index} names a different task manifest",
        )
        expected_repo = task_contract["repo"]
        require(
            configured.get("url") == expected_repo["url"]
            and configured.get("ref") == expected_repo["ref"]
            and configured.get("languages") == expected_repo.get("languages", [])
            and manifest_repo.get("url") == expected_repo["url"]
            and manifest_repo.get("ref") == expected_repo["ref"]
            and manifest_repo.get("workspace_root") == expected_repo.get("workspace_root"),
            f"packet quality row {index} repository identity differs from its checked-in task",
        )
        cell = (repo, task_id, mode, repeat)
        require(
            cell not in observed_cells,
            f"packet quality rows duplicate repeat {repeat} for {repo}/{task_id}/{mode}",
        )
        observed_cells.add(cell)
        passing_rows += 1

    require(
        observed_cells == expected_cells,
        "packet quality rows do not exactly cover the checked-in repo/task/mode/repeat matrix",
    )
    pass_rate = passing_rows / len(rows)
    return {
        "artifact": {
            "name": "packet-runtime-summary.json",
            "sha256": artifact_sha256,
        },
        "evaluation_contract": RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "source_commit": source["commit"],
        "source_tree": source["tree"],
        "corpus_id": RELEASE_QUALITY_CORPUS_ID,
        "holdout_manifest_set_sha256": holdout_manifest_set_sha256,
        "repeats": repeats,
        "row_count": len(rows),
        "passing_row_count": passing_rows,
        "publishable_packet_pass_rate": pass_rate,
    }


def derive_scenario_assertions(
    scenario_id: str,
    *,
    observations_by_kind: dict[str, list[dict]],
    process_observations: list[dict],
    invocations: list[dict],
    control_actions: list[str],
    same_account: dict,
    materialization: dict,
) -> dict[str, bool]:
    def transition(kind: str, expected_keys: set[str] | None = None) -> dict:
        matches = observations_by_kind.get(kind, [])
        require(
            len(matches) == 1,
            f"qualification scenario {scenario_id} omitted or duplicated transition {kind}",
        )
        values = matches[0]["values"]
        require(isinstance(values, dict), f"qualification transition {kind} values are malformed")
        if expected_keys is not None:
            require_exact_keys(values, expected_keys, f"qualification transition {kind} values")
        return values

    def scheduler(kind: str) -> dict:
        values = transition(
            kind,
            {
                "query_capacity",
                "query_depth",
                "bulk_capacity",
                "bulk_depth",
                "active_request_count",
                "lease_count",
                "active_request_class",
            },
        )
        for field in (
            "query_capacity",
            "query_depth",
            "bulk_capacity",
            "bulk_depth",
            "active_request_count",
            "lease_count",
        ):
            require_nonnegative_int(values[field], f"qualification transition {kind}.{field}")
        require(
            values["active_request_class"] in {None, "query", "bulk"},
            f"qualification transition {kind} has an invalid active request class",
        )
        return values

    def replay_attempts(
        values: dict,
        *,
        old_server_instance_id: str,
        new_server_instance_id: str,
    ) -> list[dict]:
        attempts = values["wire_attempts"]
        require(
            values["wire_attempt_count"] == 2
            and isinstance(attempts, list)
            and len(attempts) == 2,
            "replay evidence must contain exactly the original RPC and one replay",
        )
        for index, attempt in enumerate(attempts, start=1):
            require(isinstance(attempt, dict), "replay attempt is malformed")
            require_exact_keys(
                attempt,
                {
                    "ordinal",
                    "request_id",
                    "server_instance_id",
                    "submitted_ns",
                    "completed_ns",
                    "outcome",
                },
                f"replay attempt {index}",
            )
            require(attempt["ordinal"] == index, "replay attempt ordinal is not exact")
            require(
                isinstance(attempt["request_id"], str) and bool(attempt["request_id"]),
                "replay attempt request ID is missing",
            )
            submitted = require_nonnegative_int(
                attempt["submitted_ns"], f"replay attempt {index} submitted_ns"
            )
            completed = require_nonnegative_int(
                attempt["completed_ns"], f"replay attempt {index} completed_ns"
            )
            require(completed >= submitted, "replay attempt clock moved backwards")
        require(
            attempts[0]["request_id"] != attempts[1]["request_id"]
            and attempts[0]["server_instance_id"] == old_server_instance_id
            and attempts[0]["outcome"] == "server_loss"
            and attempts[1]["server_instance_id"] == new_server_instance_id
            and attempts[1]["outcome"] == "completed",
            "replay attempts do not bind the old loss and exact replacement completion",
        )
        return attempts

    def retry_state(value: object, field: str) -> dict:
        require(isinstance(value, dict), f"{field} is malformed")
        expected = {
            "code",
            "message_head",
            "retry_class",
            "retry_after_ms",
            "retry_condition",
        }
        if "capacity" in value:
            expected.add("capacity")
        require_exact_keys(value, expected, field)
        require_nonempty_string(value["code"], f"{field}.code")
        require_nonempty_string(value["message_head"], f"{field}.message_head")
        require_nonempty_string(value["retry_class"], f"{field}.retry_class")
        require_nonnegative_int(value["retry_after_ms"], f"{field}.retry_after_ms")
        require_nonempty_string(value["retry_condition"], f"{field}.retry_condition")
        require(
            value["retry_class"]
            in {
                "after_capacity_change",
                "after_delay",
                "after_owner_idle",
                "after_server_change",
                "none",
                "same_rpc_once",
                "terminal",
            },
            f"{field}.retry_class is outside the protocol contract",
        )
        return value

    snapshots = [
        observation["snapshot"]
        for observation in process_observations
        if observation.get("snapshot") is not None
    ]
    snapshot_instances = {
        snapshot["process"]["server_instance_id"] for snapshot in snapshots
    }
    snapshot_authorities = {
        (
            snapshot["authority"]["lifetime_authority_id"],
            snapshot["authority"]["listener_id"],
        )
        for snapshot in snapshots
    }
    snapshot_engines = {
        (
            snapshot["engine"]["engine_owner_id"],
            snapshot["engine"]["native_worker_id"],
            snapshot["engine"]["load_generation"],
            snapshot["engine"]["model_load_count"],
        )
        for snapshot in snapshots
        if snapshot.get("engine") is not None
    }

    assertions: dict[str, bool]
    if scenario_id == "client_death":
        active = scheduler("dead_client_work_observed")
        continued = transition("other_client_continued", {"project_identity_sha256"})
        terminated = transition("client_terminated", {"termination"})
        reclaimed = scheduler("dead_client_work_reclaimed")
        post = transition("post_reclaim_other_client_query", {"server_instance_id"})
        assertions = {
            "dead_client_queue_and_leases_reclaimed": (
                active["query_depth"] > 0
                and active["bulk_depth"] > 0
                and active["active_request_count"] > 0
                and active["lease_count"] > 0
                and reclaimed["query_depth"] == 0
                and reclaimed["bulk_depth"] == 0
                and reclaimed["active_request_count"] == 0
                and reclaimed["lease_count"] == 0
                and terminated["termination"] == "terminated"
            ),
            "other_client_continues": (
                HEX_SHA256.fullmatch(str(continued["project_identity_sha256"])) is not None
                and post["server_instance_id"] in snapshot_instances
            ),
            "no_server_replacement": len(snapshot_instances) == 1,
        }
    elif scenario_id == "cold_race":
        election_witnesses = {
            phase: [
                observation
                for observation in process_observations
                if observation.get("phase") == phase
            ]
            for phase in ("cold_race_first", "cold_race_second")
        }
        require(
            all(len(witnesses) == 1 for witnesses in election_witnesses.values()),
            "cold race must retain exactly one post-reset snapshot from each process",
        )
        election_snapshots = [
            election_witnesses[phase][0]["snapshot"]
            for phase in ("cold_race_first", "cold_race_second")
        ]
        require(
            all(
                isinstance(snapshot, dict) and snapshot.get("engine") is not None
                for snapshot in election_snapshots
            ),
            "cold race post-reset snapshots must retain engine identity",
        )
        election_instances = {
            snapshot["process"]["server_instance_id"]
            for snapshot in election_snapshots
        }
        election_authorities = {
            (
                snapshot["authority"]["lifetime_authority_id"],
                snapshot["authority"]["listener_id"],
            )
            for snapshot in election_snapshots
        }
        election_engines = {
            (
                snapshot["engine"]["engine_owner_id"],
                snapshot["engine"]["native_worker_id"],
                snapshot["engine"]["load_generation"],
                snapshot["engine"]["model_load_count"],
            )
            for snapshot in election_snapshots
        }
        independent = transition(
            "two_independent_processes",
            {
                "first_pid",
                "second_pid",
                "first_project_identity_sha256",
                "second_project_identity_sha256",
                "first_transport_peer_verified",
                "second_transport_peer_verified",
            },
        )
        converged = transition(
            "single_server_convergence",
            {"server_instance_id", "lifetime_authority_id"},
        )
        hosts = same_account.get("plugin_hosts") if isinstance(same_account, dict) else None
        assertions = {
            "two_independent_plugin_hosts": (
                require_positive_int(independent["first_pid"], "cold race first pid")
                != require_positive_int(independent["second_pid"], "cold race second pid")
                and independent["first_transport_peer_verified"] is True
                and independent["second_transport_peer_verified"] is True
            ),
            "same_os_account": (
                same_account.get("relation") == "same_os_account"
                and isinstance(hosts, list)
                and len(hosts) == 2
            ),
            "different_repositories": (
                independent["first_project_identity_sha256"]
                != independent["second_project_identity_sha256"]
                and all(
                    HEX_SHA256.fullmatch(str(independent[field])) is not None
                    for field in (
                        "first_project_identity_sha256",
                        "second_project_identity_sha256",
                    )
                )
            ),
            "one_lifetime_authority": (
                len(election_authorities) == 1
                and converged["lifetime_authority_id"]
                == next(iter(election_authorities))[0]
            ),
            "one_listener": len({identity[1] for identity in election_authorities}) == 1,
            "one_server": (
                len(election_instances) == 1
                and converged["server_instance_id"] == next(iter(election_instances))
            ),
            "one_engine_owner": len({identity[0] for identity in election_engines}) == 1,
            "one_native_worker": len({identity[1] for identity in election_engines}) == 1,
            "one_load_generation": len({identity[2] for identity in election_engines}) == 1,
            "one_model_load": (
                len(election_engines) == 1 and next(iter(election_engines))[3] == 1
            ),
        }
    elif scenario_id == "frozen_owner":
        bounded = transition(
            "bounded_owner_unresponsive",
            {
                "started_ns",
                "finished_ns",
                "error_code",
                "timeout_ms",
                "clock_domain",
                "clock_boot_id",
                "retry",
            },
        )
        stable = transition(
            "owner_identity_stable",
            {
                "server_instance_id",
                "lifetime_authority_id",
                "listener_id",
                "pid",
                "process_start_id",
                "post_release_query_succeeded",
            },
        )
        started = require_nonnegative_int(bounded["started_ns"], "frozen owner started_ns")
        finished = require_nonnegative_int(bounded["finished_ns"], "frozen owner finished_ns")
        timeout_ms = require_positive_int(bounded["timeout_ms"], "frozen owner timeout_ms")
        stable_pid = require_positive_int(stable["pid"], "frozen owner stable pid")
        retry = retry_state(bounded["retry"], "frozen owner retry")
        stable_identity = (
            len(snapshot_instances) == 1
            and stable["server_instance_id"] == next(iter(snapshot_instances))
            and len(snapshot_authorities) == 1
            and (
                stable["lifetime_authority_id"],
                stable["listener_id"],
            )
            == next(iter(snapshot_authorities))
            and all(
                snapshot["process"]["pid"] == stable_pid
                and snapshot["process"]["process_start_id"]
                == stable["process_start_id"]
                for snapshot in snapshots
            )
            and stable["post_release_query_succeeded"] is True
        )
        assertions = {
            "owner_unresponsive_is_bounded": (
                finished >= started
                and finished - started <= timeout_ms * 1_000_000
                and bounded["clock_domain"] == "awake_monotonic"
                and bool(bounded["clock_boot_id"])
                and bounded["error_code"] == "embedding_server_owner_unresponsive"
                and retry["code"] == bounded["error_code"]
                and retry["retry_class"] == "after_server_change"
                and bool(retry["retry_condition"])
            ),
            "authority_retained": stable_identity,
            "no_unlink": stable_identity,
            "no_pid_kill": stable_identity,
            "no_takeover": stable_identity,
            "no_second_engine": len(snapshot_engines) == 1,
        }
    elif scenario_id == "incompatible_owner":
        active = transition(
            "active_owner_rejected",
            {"compatibility_evidence", "error_code", "retry"},
        )
        idle = transition(
            "idle_owner_draining",
            {"compatibility_evidence", "error_code", "retry"},
        )
        replacement = transition(
            "compatible_replacement",
            {"old_server_instance_id", "new_server_instance_id"},
        )
        replaced = (
            replacement["old_server_instance_id"] != replacement["new_server_instance_id"]
            and {
                replacement["old_server_instance_id"],
                replacement["new_server_instance_id"],
            }
            <= snapshot_instances
        )
        active_retry = retry_state(active["retry"], "incompatible active retry")
        idle_retry = retry_state(idle["retry"], "incompatible idle retry")
        assertions = {
            "idle_owner_drains": (
                idle["compatibility_evidence"] == "injected_contract_mismatch"
                and idle["error_code"] == "embedding_server_draining"
                and idle_retry["code"] == idle["error_code"]
                and idle_retry["retry_class"] == "after_owner_idle"
                and idle_retry["retry_after_ms"] == 0
                and idle_retry["retry_condition"]
                == "the incompatible server exits while fully idle"
                and replaced
            ),
            "active_owner_returns_typed_retry": (
                active["compatibility_evidence"] == "injected_contract_mismatch"
                and active["error_code"] == "embedding_server_incompatible_active_owner"
                and active_retry["code"] == active["error_code"]
                and active_retry["retry_class"] == "after_owner_idle"
                and active_retry["retry_after_ms"] == 0
                and active_retry["retry_condition"]
                == "the incompatible server exits while fully idle"
            ),
            "one_authority": len(snapshot_authorities) <= 2 and replaced,
            "one_engine_maximum": len(snapshot_instances) == 2 and replaced,
        }
    elif scenario_id == "mixed_queue":
        saturated = scheduler("queues_saturated")
        selected = scheduler("query_selected_before_bulk_backlog")
        capacity = transition("typed_capacity_retry_observed", {"query_65th", "bulk_65th"})
        class_orders = transition(
            "per_class_fifo_observed",
            {
                "query_expected_queue_insertion_request_ids",
                "query_native_completed_request_ids",
                "query_native_completion_sequences",
                "bulk_expected_queue_insertion_request_ids",
                "bulk_native_completed_request_ids",
                "bulk_native_completion_sequences",
            },
        )
        project_orders = transition(
            "global_fifo_across_projects",
            {
                "query_expected_queue_insertion_project_identities",
                "query_native_completed_project_identities",
                "bulk_expected_queue_insertion_project_identities",
                "bulk_native_completed_project_identities",
            },
        )
        preference = transition(
            "query_preference_observed",
            {
                "first_query_request_id",
                "first_query_native_completion_sequence",
                "first_bulk_request_id",
                "first_bulk_native_completion_sequence",
            },
        )
        resumed = transition(
            "bulk_resumed",
            {
                "last_query_request_id",
                "last_query_native_completion_sequence",
                "last_bulk_request_id",
                "last_bulk_native_completion_sequence",
            },
        )
        typed_capacity = True
        for queue_class in ("query", "bulk"):
            record = capacity[f"{queue_class}_65th"]
            pressure = record.get("error", {}).get("capacity") if isinstance(record, dict) else None
            typed_capacity = typed_capacity and (
                isinstance(pressure, dict)
                and pressure.get("queue_class") == queue_class
                and pressure.get("capacity") == 64
                and pressure.get("depth") == 64
                and bool(pressure.get("retry_condition"))
            )
        fifo = all(
            class_orders[f"{queue_class}_expected_queue_insertion_request_ids"]
            == class_orders[f"{queue_class}_native_completed_request_ids"]
            and isinstance(
                class_orders[f"{queue_class}_expected_queue_insertion_request_ids"], list
            )
            and bool(class_orders[f"{queue_class}_expected_queue_insertion_request_ids"])
            and isinstance(class_orders[f"{queue_class}_native_completion_sequences"], list)
            and bool(class_orders[f"{queue_class}_native_completion_sequences"])
            and all(
                isinstance(sequence, int)
                and not isinstance(sequence, bool)
                and sequence > 0
                for sequence in class_orders[f"{queue_class}_native_completion_sequences"]
            )
            and class_orders[f"{queue_class}_native_completion_sequences"]
            == sorted(class_orders[f"{queue_class}_native_completion_sequences"])
            and len(set(class_orders[f"{queue_class}_native_completion_sequences"]))
            == len(class_orders[f"{queue_class}_native_completion_sequences"])
            for queue_class in ("query", "bulk")
        )
        global_fifo = all(
            project_orders[f"{queue_class}_expected_queue_insertion_project_identities"]
            == project_orders[f"{queue_class}_native_completed_project_identities"]
            and len(
                set(project_orders[f"{queue_class}_expected_queue_insertion_project_identities"])
            )
            == 2
            for queue_class in ("query", "bulk")
        )
        assertions = {
            "query_and_bulk_capacities_are_64": (
                saturated["query_capacity"] == saturated["query_depth"] == 64
                and saturated["bulk_capacity"] == saturated["bulk_depth"] == 64
            ),
            "fifo_within_each_class": fifo,
            "query_preferred_between_bulk_batches": (
                selected["active_request_class"] == "query"
                and selected["bulk_depth"] > 0
                and preference["first_query_native_completion_sequence"]
                < preference["first_bulk_native_completion_sequence"]
            ),
            "bulk_resumes_when_query_queue_permits": (
                resumed["last_bulk_native_completion_sequence"]
                > resumed["last_query_native_completion_sequence"]
            ),
            "no_project_or_scope_round_robin": global_fifo,
            "typed_retry_names_useful_condition": typed_capacity,
            "no_project_or_request_text_leakage": all(
                all(
                    HEX_SHA256.fullmatch(str(value)) is not None
                    for value in values
                )
                for key, values in project_orders.items()
                if key.endswith("_project_identities")
            ),
        }
    elif scenario_id == "server_crash":
        active = scheduler("inflight_request_observed")
        replacement = transition(
            "server_replaced",
            {"old_server_instance_id", "new_server_instance_id"},
        )
        replay = transition(
            "query_replayed",
            {
                "logical_operation_count",
                "wire_attempt_count",
                "wire_attempts",
            },
        )
        attempts = replay_attempts(
            replay,
            old_server_instance_id=replacement["old_server_instance_id"],
            new_server_instance_id=replacement["new_server_instance_id"],
        )
        assertions = {
            "one_replacement_server": (
                active["active_request_class"] == "query"
                and replacement["old_server_instance_id"]
                != replacement["new_server_instance_id"]
                and [attempt["server_instance_id"] for attempt in attempts]
                == [
                    replacement["old_server_instance_id"],
                    replacement["new_server_instance_id"],
                ]
            ),
            "pure_embedding_rpc_replayed_at_most_once": (
                replay["logical_operation_count"] == 1
                and replay["wire_attempt_count"] <= 2
                and sum(attempt["outcome"] == "completed" for attempt in attempts) == 1
            ),
        }
    elif scenario_id == "true_idle_respawn":
        active = scheduler("anti_idle_work_observed")
        preserved = transition(
            "owner_preserved_across_idle_boundary",
            {
                "held_started_ns",
                "held_observed_ns",
                "contract_idle_timeout_ms",
                "server_instance_id",
            },
        )
        reclaimed = scheduler("anti_idle_work_reclaimed")
        waited = transition(
            "true_idle_wait",
            {
                "server_idle_epoch_ns",
                "server_idle_elapsed_before_client_wait_ns",
                "client_wait_required_ns",
                "client_wait_elapsed_ns",
                "contract_idle_timeout_ms",
                "clock_boot_id",
            },
        )
        idle_surfaces = transition(
            "idle_surfaces_exercised",
            {
                "diagnostic_count",
                "idle_connection_close_count",
                "last_diagnostic_client_elapsed_ns",
                "last_idle_connection_close_client_elapsed_ns",
            },
        )
        absent = transition("owner_absent_after_true_idle", {"old_server_instance_id"})
        respawned = transition(
            "server_respawned",
            {
                "new_server_instance_id",
                "load_generation",
                "model_load_count",
                "materialized_model_sha256",
                "materialized_reused",
            },
        )
        absent_transition_observation = observations_by_kind[
            "owner_absent_after_true_idle"
        ][0]
        respawn_transition_observation = observations_by_kind["server_respawned"][0]
        timeout_ms = require_positive_int(
            waited["contract_idle_timeout_ms"],
            "true idle contract timeout",
        )
        diagnostic_count = require_positive_int(
            idle_surfaces["diagnostic_count"],
            "true idle diagnostic count",
        )
        idle_connection_close_count = require_positive_int(
            idle_surfaces["idle_connection_close_count"],
            "true idle connection close count",
        )
        last_diagnostic_client_elapsed_ns = require_nonnegative_int(
            idle_surfaces["last_diagnostic_client_elapsed_ns"],
            "true idle last diagnostic ns",
        )
        last_idle_connection_close_client_elapsed_ns = require_nonnegative_int(
            idle_surfaces["last_idle_connection_close_client_elapsed_ns"],
            "true idle last connection close ns",
        )
        absent_observations = [
            observation
            for observation in process_observations
            if observation.get("phase") == "true_idle_after_wait"
        ]
        require(
            len(absent_observations) == 1
            and absent_observations[0].get("snapshot") is None,
            "true idle must retain exactly one absent-owner witness",
        )
        respawn_observations = [
            observation
            for observation in process_observations
            if observation.get("phase") == "true_idle_respawned"
        ]
        require(
            len(respawn_observations) == 1
            and isinstance(respawn_observations[0].get("snapshot"), dict)
            and respawn_observations[0]["snapshot"].get("engine") is not None,
            "true idle must retain exactly one replacement-engine witness",
        )
        absent_observed_ns = require_nonnegative_int(
            absent_observations[0].get("observed_ns"),
            "true idle absent-owner witness time",
        )
        respawn_observed_ns = require_nonnegative_int(
            respawn_observations[0].get("observed_ns"),
            "true idle replacement witness time",
        )
        respawn_snapshot = respawn_observations[0]["snapshot"]
        respawn_engine = respawn_snapshot["engine"]
        absent_transition_ns = require_nonnegative_int(
            absent_transition_observation.get("observed_ns"),
            "true idle absence transition time",
        )
        respawn_transition_ns = require_nonnegative_int(
            respawn_transition_observation.get("observed_ns"),
            "true idle respawn transition time",
        )
        post_absence_invocations = [
            invocation
            for invocation in invocations
            if isinstance(invocation.get("started_ns"), int)
            and not isinstance(invocation.get("started_ns"), bool)
            and absent_transition_ns <= invocation["started_ns"]
            <= respawn_transition_ns
        ]
        absent_observed = (
            len(absent_observations) == 1
            and absent_observations[0].get("snapshot") is None
        )
        respawn_load_generation = require_positive_int(
            respawned["load_generation"],
            "true idle respawn load generation",
        )
        respawn_model_load_count = require_positive_int(
            respawned["model_load_count"],
            "true idle respawn model load count",
        )
        respawn_materialized_sha256 = require_sha256(
            respawned["materialized_model_sha256"],
            "true idle respawn materialized model",
        )
        require_nonnegative_int(
            waited["server_idle_epoch_ns"], "true idle server epoch"
        )
        server_idle_elapsed_before_client_wait_ns = require_nonnegative_int(
            waited["server_idle_elapsed_before_client_wait_ns"],
            "true idle server elapsed before local wait",
        )
        client_wait_required_ns = require_nonnegative_int(
            waited["client_wait_required_ns"], "true idle client wait required"
        )
        client_wait_elapsed_ns = require_nonnegative_int(
            waited["client_wait_elapsed_ns"], "true idle client wait elapsed"
        )
        assertions = {
            "queued_active_and_leased_work_prevent_exit": (
                active["query_depth"] > 0
                and active["bulk_depth"] > 0
                and active["active_request_count"] > 0
                and active["lease_count"] > 0
                and preserved["server_instance_id"] == absent["old_server_instance_id"]
                and preserved["held_observed_ns"] - preserved["held_started_ns"]
                >= preserved["contract_idle_timeout_ms"] * 1_000_000
            ),
            "idle_connections_and_diagnostics_do_not_extend_idle": (
                diagnostic_count >= 2
                and idle_connection_close_count >= 2
                and last_diagnostic_client_elapsed_ns
                >= timeout_ms * 500_000
                and last_idle_connection_close_client_elapsed_ns
                >= timeout_ms * 500_000
                and bool(waited["clock_boot_id"])
                and absent_observed
            ),
            "exit_after_60000_awake_ms": (
                timeout_ms == 60_000
                and server_idle_elapsed_before_client_wait_ns
                + client_wait_required_ns
                >= timeout_ms * 1_000_000
                and client_wait_elapsed_ns >= client_wait_required_ns
                and reclaimed["query_depth"] == 0
                and reclaimed["bulk_depth"] == 0
                and reclaimed["active_request_count"] == 0
                and reclaimed["lease_count"] == 0
                and absent_observed
            ),
            "next_product_operation_respawns_without_consent": (
                absent["old_server_instance_id"] != respawned["new_server_instance_id"]
                and absent_observed_ns <= absent_transition_ns
                and len(post_absence_invocations) == 1
                and post_absence_invocations[0].get("operation") == "query"
                and post_absence_invocations[0].get("exit_code") == 0
                and post_absence_invocations[0].get("termination") == "exited"
                and isinstance(post_absence_invocations[0].get("finished_ns"), int)
                and not isinstance(
                    post_absence_invocations[0].get("finished_ns"), bool
                )
                and post_absence_invocations[0]["started_ns"]
                <= post_absence_invocations[0]["finished_ns"]
                <= respawn_observed_ns <= respawn_transition_ns
                and respawn_snapshot["process"]["server_instance_id"]
                == respawned["new_server_instance_id"]
            ),
            "verified_materialization_reused": (
                isinstance(materialization, dict)
                and respawn_materialized_sha256
                == require_sha256(
                    materialization.get("sha256"),
                    "retained materialized model",
                )
                and respawned["materialized_reused"] is True
                and respawn_load_generation
                == respawn_engine["load_generation"]
                and respawn_model_load_count
                == respawn_engine["model_load_count"] == 1
            ),
        }
    else:
        require(scenario_id == "worker_stall", f"unknown qualification scenario {scenario_id}")
        active = scheduler("stalled_request_observed")
        fail_stop = transition(
            "watchdog_fail_stop_observed",
            {
                "old_pid",
                "old_server_instance_id",
                "wire_attempt_count",
                "wire_attempts",
                "watchdog_marker_sha256",
                "watchdog_reason",
                "watchdog_observed_ns",
                "watchdog_last_progress_ns",
                "watchdog_progress_sequence",
                "hard_native_no_progress_ms",
                "watchdog_cadence_ms",
            },
        )
        replacement = transition(
            "post_stall_replacement",
            {"new_server_instance_id"},
        )
        survivor = transition(
            "unrelated_process_survived",
            {"pid", "process_start_id", "new_server_instance_id"},
        )
        old_pid = require_positive_int(fail_stop["old_pid"], "worker stall old pid")
        marker_sha256 = require_sha256(
            fail_stop["watchdog_marker_sha256"], "worker stall watchdog marker"
        )
        watchdog_observed_ns = require_nonnegative_int(
            fail_stop["watchdog_observed_ns"], "worker stall watchdog observed_ns"
        )
        watchdog_last_progress_ns = require_nonnegative_int(
            fail_stop["watchdog_last_progress_ns"],
            "worker stall watchdog last progress",
        )
        hard_no_progress_ms = require_positive_int(
            fail_stop["hard_native_no_progress_ms"],
            "worker stall hard no-progress bound",
        )
        watchdog_cadence_ms = require_positive_int(
            fail_stop["watchdog_cadence_ms"], "worker stall watchdog cadence"
        )
        require_nonnegative_int(
            fail_stop["watchdog_progress_sequence"],
            "worker stall watchdog progress sequence",
        )
        attempts = replay_attempts(
            fail_stop,
            old_server_instance_id=fail_stop["old_server_instance_id"],
            new_server_instance_id=replacement["new_server_instance_id"],
        )
        replacement_observed = (
            replacement["new_server_instance_id"] in snapshot_instances
            and all(snapshot["process"]["pid"] != old_pid for snapshot in snapshots[-1:])
        )
        assertions = {
            "independent_watchdog_fail_stops_server": (
                active["active_request_class"] == "bulk"
                and bool(marker_sha256)
                and fail_stop["watchdog_reason"] == "embedding_engine_stalled"
                and watchdog_observed_ns >= watchdog_last_progress_ns
                and watchdog_observed_ns - watchdog_last_progress_ns
                >= hard_no_progress_ms * 1_000_000
                and watchdog_cadence_ms < hard_no_progress_ms
                and attempts[0]["outcome"] == "server_loss"
                and replacement_observed
            ),
            "unrelated_process_survives": (
                replacement_observed
                and require_positive_int(survivor["pid"], "worker stall survivor pid")
                != old_pid
                and bool(survivor["process_start_id"])
                and survivor["new_server_instance_id"]
                == replacement["new_server_instance_id"]
                and any(
                    invocation.get("pid") == survivor["pid"]
                    and invocation.get("process_start_id")
                    == survivor["process_start_id"]
                    and invocation.get("operation") == "query"
                    and invocation.get("termination") == "exited"
                    for invocation in invocations
                )
            ),
            "pure_embedding_rpc_replayed_at_most_once": (
                fail_stop["wire_attempt_count"] <= 2
                and sum(attempt["outcome"] == "completed" for attempt in attempts) == 1
            ),
        }
    failed = sorted(name for name, value in assertions.items() if value is not True)
    require(
        not failed,
        f"qualification scenario {scenario_id} raw evidence failed assertions: {', '.join(failed)}",
    )
    return assertions


def qualification_artifact(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    package: dict,
    same_account: dict,
    materialization: dict,
    nonce_sha256: str,
    forbidden_values: list[str],
) -> tuple[dict, dict]:
    require(
        isinstance(summary, dict),
        f"qualification scenario {scenario_id} summary is malformed",
    )
    require_exact_keys(
        summary,
        {
            "artifact",
            "process_count",
            "control_event_count",
            "process_observation_count",
            "observation_count",
            "event_count",
        },
        f"qualification scenario {scenario_id} summary",
    )
    name = require_nonempty_string(
        summary["artifact"],
        f"qualification scenario {scenario_id} artifact",
    )
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and relative.suffix == ".json",
        f"qualification scenario {scenario_id} artifact must be a JSON basename",
    )
    path = artifact_root / relative
    require(path.is_file() and not path.is_symlink(), f"qualification artifact is missing or unsafe: {name}")
    require(
        path.resolve().parent == artifact_root.resolve(),
        f"qualification artifact escaped its private output directory: {name}",
    )
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            f"qualification artifact {name} leaked private request material",
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"qualification artifact {name} is not valid JSON: {exc}") from exc
    require(isinstance(payload, dict), f"qualification artifact {name} must be an object")
    require_exact_keys(
        payload,
        {
            "schema_version",
            "scenario",
            "contracts",
            "orchestration",
            "control_events",
            "process_observations",
            "observations",
            "events",
        },
        f"qualification artifact {name}",
    )
    require(payload["schema_version"] == 3, f"qualification artifact {name} schema is unsupported")
    require(payload["scenario"] == scenario_id, f"qualification artifact {name} names the wrong scenario")
    require(payload["contracts"] == contracts, f"qualification artifact {name} used different contracts")

    orchestration = payload["orchestration"]
    require(
        isinstance(orchestration, dict),
        f"qualification artifact {name} orchestration is malformed",
    )
    require_exact_keys(
        orchestration,
        {"started_ns", "finished_ns", "process_invocations"},
        f"qualification artifact {name} orchestration",
    )
    started_ns = require_nonnegative_int(
        orchestration["started_ns"],
        f"qualification artifact {name} orchestration.started_ns",
    )
    finished_ns = require_nonnegative_int(
        orchestration["finished_ns"],
        f"qualification artifact {name} orchestration.finished_ns",
    )
    require(
        finished_ns >= started_ns,
        f"qualification artifact {name} orchestration moved backwards",
    )
    invocations = orchestration["process_invocations"]
    require(
        isinstance(invocations, list),
        f"qualification artifact {name} process invocations are malformed",
    )
    invocation_ids: set[str] = set()
    for index, invocation in enumerate(invocations):
        require(
            isinstance(invocation, dict),
            f"qualification artifact {name} process invocation {index} is malformed",
        )
        require_exact_keys(
            invocation,
            {
                "invocation_id",
                "operation",
                "project_identity_sha256",
                "pid",
                "process_start_id",
                "started_ns",
                "finished_ns",
                "exit_code",
                "termination",
            },
            f"qualification artifact {name} process invocation {index}",
        )
        invocation_id = require_nonempty_string(
            invocation["invocation_id"],
            f"qualification artifact {name} process invocation {index}.invocation_id",
        )
        require(
            invocation_id not in invocation_ids,
            f"qualification artifact {name} duplicated invocation {invocation_id}",
        )
        invocation_ids.add(invocation_id)
        require_nonempty_string(
            invocation["operation"],
            f"qualification artifact {name} process invocation {index}.operation",
        )
        require_sha256(
            invocation["project_identity_sha256"],
            f"qualification artifact {name} process invocation {index}.project_identity_sha256",
        )
        require_positive_int(
            invocation["pid"],
            f"qualification artifact {name} process invocation {index}.pid",
        )
        require_nonempty_string(
            invocation["process_start_id"],
            f"qualification artifact {name} process invocation {index}.process_start_id",
        )
        invocation_started = require_nonnegative_int(
            invocation["started_ns"],
            f"qualification artifact {name} process invocation {index}.started_ns",
        )
        invocation_finished = require_nonnegative_int(
            invocation["finished_ns"],
            f"qualification artifact {name} process invocation {index}.finished_ns",
        )
        require(
            started_ns <= invocation_started <= invocation_finished <= finished_ns,
            f"qualification artifact {name} process invocation {index} escaped its block",
        )
        require(
            invocation["exit_code"] is None
            or (
                isinstance(invocation["exit_code"], int)
                and not isinstance(invocation["exit_code"], bool)
            ),
            f"qualification artifact {name} process invocation {index}.exit_code is invalid",
        )
        require(
            invocation["termination"] in {"exited", "terminated"},
            f"qualification artifact {name} process invocation {index}.termination is invalid",
        )

    def validate_clock(clock: object, field: str, *, observed: bool) -> None:
        require(isinstance(clock, dict), f"{field} is malformed")
        expected = {"domain", "api", "boot_id", "resolution_ns"}
        if observed:
            expected = {"domain", "api", "boot_id", "observed_ns"}
        require_exact_keys(clock, expected, field)
        require(clock["domain"] == "awake_monotonic", f"{field} used the wrong clock domain")
        require_nonempty_string(clock["api"], f"{field}.api")
        require_nonempty_string(clock["boot_id"], f"{field}.boot_id")
        numeric = "observed_ns" if observed else "resolution_ns"
        require_nonnegative_int(clock[numeric], f"{field}.{numeric}")

    def validate_snapshot(snapshot: object, field: str) -> None:
        require(isinstance(snapshot, dict), f"{field} is malformed")
        required = {
            "schema_version",
            "event_sequence",
            "lifecycle",
            "clock",
            "protocol",
            "authority",
            "process",
            "scheduler",
        }
        require(
            required <= set(snapshot)
            and set(snapshot) <= required | {"engine", "failure"},
            f"{field} fields differ from the raw snapshot contract",
        )
        require(snapshot["schema_version"] == 1, f"{field} schema is unsupported")
        require_nonnegative_int(snapshot["event_sequence"], f"{field}.event_sequence")
        require(snapshot["lifecycle"] in SERVER_LIFECYCLES, f"{field} lifecycle is invalid")
        validate_clock(snapshot["clock"], f"{field}.clock", observed=False)
        protocol = snapshot["protocol"]
        require(isinstance(protocol, dict), f"{field}.protocol is malformed")
        for contract_field, expected in contracts.items():
            require(
                protocol.get(contract_field) == expected,
                f"{field}.protocol.{contract_field} is stale",
            )
        authority = snapshot["authority"]
        process = snapshot["process"]
        scheduler = snapshot["scheduler"]
        require(
            isinstance(authority, dict)
            and isinstance(process, dict)
            and isinstance(scheduler, dict),
            f"{field} omitted server identity",
        )
        require_exact_keys(
            process,
            {
                "server_instance_id",
                "pid",
                "process_start_id",
                "executable_sha256",
                "executable_version",
            },
            f"{field}.process",
        )
        for identity_field in (
            "endpoint_namespace_id",
            "lifetime_authority_id",
            "listener_id",
        ):
            require_nonempty_string(authority.get(identity_field), f"{field}.authority.{identity_field}")
        require_nonempty_string(
            process.get("server_instance_id"),
            f"{field}.process.server_instance_id",
        )
        require_positive_int(process.get("pid"), f"{field}.process.pid")
        require_nonempty_string(process.get("process_start_id"), f"{field}.process.process_start_id")
        require(
            process.get("executable_sha256") == package["executable_sha256"]
            and process.get("executable_version") == package["release_version"],
            f"{field}.process does not match the exact packaged executable",
        )
        require(
            scheduler.get("query_capacity") == 64 and scheduler.get("bulk_capacity") == 64,
            f"{field} queue capacities differ from the bound contract",
        )
        engine = snapshot.get("engine")
        if engine is not None:
            require(isinstance(engine, dict), f"{field}.engine is malformed")
            for identity_field in ("engine_owner_id", "native_worker_id"):
                require_nonempty_string(engine.get(identity_field), f"{field}.engine.{identity_field}")
            require_positive_int(engine.get("load_generation"), f"{field}.engine.load_generation")
            require_positive_int(engine.get("model_load_count"), f"{field}.engine.model_load_count")

    control_events = payload["control_events"]
    require(
        isinstance(control_events, list),
        f"qualification artifact {name} control events are malformed",
    )
    previous_control_sequence = -1
    control_actions: list[str] = []
    for index, event in enumerate(control_events):
        require(
            isinstance(event, dict),
            f"qualification artifact {name} control event {index} is malformed",
        )
        required = {
            "schema_version",
            "sequence",
            "action",
            "status",
            "authenticated_nonce_sha256",
            "server_event_sequence",
            "clock",
        }
        require(
            required <= set(event)
            and set(event) <= required | {"snapshot", "details"},
            f"qualification artifact {name} control event {index} fields are invalid",
        )
        require(event["schema_version"] == 1, f"qualification artifact {name} control event schema is unsupported")
        sequence = require_nonnegative_int(
            event["sequence"],
            f"qualification artifact {name} control event {index}.sequence",
        )
        require(
            sequence > previous_control_sequence,
            f"qualification artifact {name} control event sequence is not increasing",
        )
        previous_control_sequence = sequence
        action = require_nonempty_string(
            event["action"],
            f"qualification artifact {name} control event {index}.action",
        )
        require(
            action
            in {
                "crash_server",
                "stall_native",
                "release_native",
                "hold_class",
                "release_class",
                "force_incompatible",
                "clear_incompatible",
                "snapshot",
                "freeze_owner",
                "release_owner",
            },
            f"qualification artifact {name} used unknown control {action}",
        )
        control_actions.append(action)
        require(
            event["status"] in {"completed", "accepted"},
            f"qualification artifact {name} control event {index} did not complete",
        )
        require(
            event["authenticated_nonce_sha256"] == nonce_sha256,
            f"qualification artifact {name} control event {index} was not authenticated",
        )
        require_nonnegative_int(
            event["server_event_sequence"],
            f"qualification artifact {name} control event {index}.server_event_sequence",
        )
        validate_clock(
            event["clock"],
            f"qualification artifact {name} control event {index}.clock",
            observed=True,
        )
        if "snapshot" in event:
            validate_snapshot(
                event["snapshot"],
                f"qualification artifact {name} control event {index}.snapshot",
            )
        if "details" in event:
            require(
                isinstance(event["details"], dict)
                and all(
                    isinstance(key, str) and isinstance(value, str)
                    for key, value in event["details"].items()
                ),
                f"qualification artifact {name} control event {index}.details is malformed",
            )

    process_observations = payload["process_observations"]
    require(
        isinstance(process_observations, list),
        f"qualification artifact {name} process observations are malformed",
    )
    observation_fields = {
        "phase",
        "observed_ns",
        "server_instance_id",
        "pid",
        "process_start_id",
        "executable_sha256",
        "executable_version",
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
        "load_generation",
        "snapshot",
    }
    for index, observation in enumerate(process_observations):
        require(
            isinstance(observation, dict),
            f"qualification artifact {name} process observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            observation_fields,
            f"qualification artifact {name} process observation {index}",
        )
        require_nonempty_string(
            observation["phase"],
            f"qualification artifact {name} process observation {index}.phase",
        )
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"qualification artifact {name} process observation {index}.observed_ns",
        )
        require(
            started_ns <= observed_ns <= finished_ns,
            f"qualification artifact {name} process observation {index} escaped its block",
        )
        snapshot = observation["snapshot"]
        if snapshot is None:
            require(
                all(
                    observation[field] is None
                    for field in observation_fields
                    - {"phase", "observed_ns", "snapshot"}
                ),
                f"qualification artifact {name} absent observation retained an identity",
            )
            continue
        validate_snapshot(
            snapshot,
            f"qualification artifact {name} process observation {index}.snapshot",
        )
        for field, expected in contracts.items():
            require(
                observation[field] == expected,
                f"qualification artifact {name} process observation {index}.{field} is stale",
            )
        require(
            observation["server_instance_id"] == snapshot["process"]["server_instance_id"]
            and observation["pid"] == snapshot["process"]["pid"]
            and observation["process_start_id"] == snapshot["process"]["process_start_id"]
            and observation["executable_sha256"] == snapshot["process"]["executable_sha256"]
            and observation["executable_version"] == snapshot["process"]["executable_version"]
            and observation["endpoint_namespace_id"]
            == snapshot["authority"]["endpoint_namespace_id"]
            and observation["lifetime_authority_id"]
            == snapshot["authority"]["lifetime_authority_id"]
            and observation["listener_id"] == snapshot["authority"]["listener_id"],
            f"qualification artifact {name} process observation {index} identity disagrees with its snapshot",
        )
        engine = snapshot.get("engine")
        require(
            observation["load_generation"]
            == (engine.get("load_generation") if isinstance(engine, dict) else None),
            f"qualification artifact {name} process observation {index} load generation disagrees with its snapshot",
        )

    observations = payload["observations"]
    require(
        isinstance(observations, list),
        f"qualification artifact {name} observations are malformed",
    )
    observations_by_kind: dict[str, list[dict]] = {}
    for index, observation in enumerate(observations):
        require(
            isinstance(observation, dict),
            f"qualification artifact {name} observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            {"sequence", "kind", "observed_ns", "values"},
            f"qualification artifact {name} observation {index}",
        )
        require(
            observation["sequence"] == index,
            f"qualification artifact {name} observation sequence is not contiguous",
        )
        kind = require_nonempty_string(
            observation["kind"],
            f"qualification artifact {name} observation {index}.kind",
        )
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"qualification artifact {name} observation {index}.observed_ns",
        )
        require(
            started_ns <= observed_ns <= finished_ns,
            f"qualification artifact {name} observation {index} escaped its block",
        )
        require(
            isinstance(observation["values"], dict),
            f"qualification artifact {name} observation {index}.values is malformed",
        )
        observations_by_kind.setdefault(kind, []).append(observation)

    required_transitions = {
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
    }[scenario_id]
    require(
        all(len(observations_by_kind.get(kind, [])) == 1 for kind in required_transitions),
        f"qualification artifact {name} omitted or duplicated required raw transitions",
    )
    required_controls = {
        "client_death": Counter({"hold_class": 2, "release_class": 2}),
        "cold_race": Counter(),
        "frozen_owner": Counter({"freeze_owner": 1, "release_owner": 1}),
        "incompatible_owner": Counter(
            {"force_incompatible": 1, "clear_incompatible": 1}
        ),
        "mixed_queue": Counter({"hold_class": 2, "release_class": 2}),
        "server_crash": Counter({"hold_class": 1, "crash_server": 1}),
        "true_idle_respawn": Counter({"hold_class": 2, "release_class": 2}),
        "worker_stall": Counter({"stall_native": 1, "release_native": 1}),
    }[scenario_id]
    actual_controls = Counter(control_actions)
    require(
        all(actual_controls[action] >= count for action, count in required_controls.items()),
        f"qualification artifact {name} omitted required authenticated controls",
    )
    if scenario_id == "cold_race":
        require(
            any(
                observation["phase"] == "cold_race_no_owner"
                and observation["snapshot"] is None
                for observation in process_observations
            ),
            f"qualification artifact {name} did not prove owner absence before the race",
        )
        independent = observations_by_kind["two_independent_processes"][0]["values"]
        require(
            independent.get("first_pid") != independent.get("second_pid")
            and independent.get("first_project_identity_sha256")
            != independent.get("second_project_identity_sha256")
            and independent.get("first_transport_peer_verified") is True
            and independent.get("second_transport_peer_verified") is True,
            f"qualification artifact {name} cold-race processes were not independent",
        )

    events = payload["events"]
    require(isinstance(events, list) and events, f"qualification artifact {name} has no correlated events")
    for index, event in enumerate(events):
        require(isinstance(event, dict), f"qualification artifact {name} event {index} is malformed")
        require_exact_keys(
            event,
            {"sequence", "source", "action", "observed_ns", "correlation_id", "values"},
            f"qualification artifact {name} event {index}",
        )
        require(
            event["sequence"] == index,
            f"qualification artifact {name} event sequence is not contiguous",
        )
        require_nonempty_string(
            event["source"],
            f"qualification artifact {name} event {index}.source",
        )
        require_nonempty_string(event["action"], f"qualification artifact {name} event {index}.action")
        observed_ns = require_nonnegative_int(
            event["observed_ns"],
            f"qualification artifact {name} event {index}.observed_ns",
        )
        require(
            started_ns <= observed_ns <= finished_ns,
            f"qualification artifact {name} event {index} escaped its block",
        )
        require(
            event["correlation_id"] is None
            or (
                isinstance(event["correlation_id"], str)
                and bool(event["correlation_id"])
            ),
            f"qualification artifact {name} event {index}.correlation_id is malformed",
        )
        require(
            isinstance(event["values"], dict),
            f"qualification artifact {name} event {index}.values is malformed",
        )

    expected_counts = {
        "process_count": len(invocations),
        "control_event_count": len(control_events),
        "process_observation_count": len(process_observations),
        "observation_count": len(observations),
        "event_count": len(events),
    }
    for field, expected in expected_counts.items():
        require(
            summary[field] == expected,
            f"qualification scenario {scenario_id} summary {field} is stale",
        )
    assertions = derive_scenario_assertions(
        scenario_id,
        observations_by_kind=observations_by_kind,
        process_observations=process_observations,
        invocations=invocations,
        control_actions=control_actions,
        same_account=same_account,
        materialization=materialization,
    )
    return (
        {"name": name, "sha256": hashlib.sha256(payload_bytes).hexdigest()},
        assertions,
    )


def selected_qualification_matrix_cell(
    protocol: dict,
    *,
    cell_id: str,
    target: str,
    proof_tier: str,
    expected_policy: str,
    expected_backend: str,
) -> dict:
    matrix = (
        protocol["calibration_matrix"]
        if proof_tier == "calibration"
        else protocol["host_package_matrix"]
    )
    require(cell_id in matrix, f"unknown qualification matrix cell {cell_id!r}")
    cell = matrix[cell_id]
    require(
        cell["asset_target"] == target
        and cell["policy"] == expected_policy
        and normalized_backend(cell["backend"]) == normalized_backend(expected_backend),
        "qualification matrix cell does not match target, policy, or backend",
    )
    require(
        cell["proof_tier"] == proof_tier,
        "qualification matrix cell does not match the requested proof tier",
    )
    return cell


def qualification_measurement_artifact(
    artifact_root: Path,
    summary: object,
    *,
    contracts: dict,
    measurement_contract: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    require(isinstance(summary, dict), "qualification measurement summary is malformed")
    require_exact_keys(
        summary,
        {"artifact", "metric_count", "sample_count"},
        "qualification measurement summary",
    )
    name = require_nonempty_string(summary["artifact"], "qualification measurement artifact")
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and name == "measurements.raw.json",
        "qualification measurement artifact must be measurements.raw.json",
    )
    path = artifact_root / relative
    require(
        path.is_file() and not path.is_symlink() and path.resolve().parent == artifact_root.resolve(),
        "qualification measurement artifact is missing or unsafe",
    )
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            "qualification measurement artifact leaked private request material",
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"qualification measurement artifact is not valid JSON: {exc}"
        ) from exc
    require(isinstance(payload, dict), "qualification measurement artifact must be an object")
    require_exact_keys(
        payload,
        {"schema_version", "contracts", "external_metrics", "metrics"},
        "qualification measurement artifact",
    )
    require(payload["schema_version"] == 2, "qualification measurement schema is unsupported")
    require(payload["contracts"] == contracts, "qualification measurements used stale contracts")
    require(
        payload["external_metrics"] == sorted(EXTERNAL_QUALIFICATION_METRICS),
        "qualification measurements changed the externally owned metric set",
    )

    protocol = measurement_contract["measurement_protocol"]
    metric_contracts = protocol["metric_contracts"]
    phase_boundaries = protocol["phase_boundaries"]
    raw_metric_names = set(protocol["required_metrics"]) - EXTERNAL_QUALIFICATION_METRICS
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=target,
        proof_tier=proof_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    metrics = payload["metrics"]
    require(
        isinstance(metrics, dict) and set(metrics) == raw_metric_names,
        "qualification measurements did not contain exactly the 12 product-path metrics",
    )
    require(
        summary["metric_count"] == len(raw_metric_names),
        "qualification measurement metric count is stale",
    )
    target_os = TARGET_CONTRACTS[target]["target_os"]
    clock_policy = protocol["clock_policy"]
    allowed_awake_apis = set(clock_policy["platform_apis"][target_os])
    suspend_contract = clock_policy["suspend_detection"]
    inclusive_api = suspend_contract["platform_apis"][target_os]
    maximum_suspend_ns = require_nonnegative_int(
        suspend_contract["maximum_inclusive_minus_awake_ns"],
        "measurement suspend-detection tolerance",
    )
    duration_metrics = raw_metric_names - {
        "bulk_documents_per_second",
        "bulk_tokens_per_second",
        "total_codestory_process_memory",
        "backend_observed_accelerator_residency",
    }
    values: dict[str, float | int] = {}
    sample_count = 0
    for metric in sorted(raw_metric_names):
        record = metrics[metric]
        require(isinstance(record, dict), f"qualification measurement {metric} is malformed")
        require_exact_keys(record, {"unit", "samples"}, f"qualification measurement {metric}")
        require(
            record["unit"] == metric_contracts[metric]["unit"],
            f"qualification measurement {metric} used the wrong unit",
        )
        samples = record["samples"]
        sample_policy = protocol["metric_sampling"][metric]
        require(
            isinstance(samples, list)
            and len(samples) == sample_policy["sample_count"],
            f"qualification measurement {metric} sample count changed",
        )
        sample_count += len(samples)
        sample_values: list[float | int] = []
        sample_ids: set[str] = set()
        server_identities: list[tuple[str, str, int]] = []
        for sample_index, sample in enumerate(samples):
            require(
                isinstance(sample, dict),
                f"qualification measurement {metric} sample {sample_index} is malformed",
            )
            require_exact_keys(
                sample,
                {
                    "sample_id",
                    "repeat",
                    "matrix_cell_id",
                    "workload_id",
                    "cache_state",
                    "residency_state",
                    "process",
                    "server_identity",
                    "clock",
                    "start",
                    "end",
                    "operands",
                    "suspend_witness",
                },
                f"qualification measurement {metric} sample {sample_index}",
            )
            sample_id = require_opaque_identifier(
                sample["sample_id"],
                f"qualification measurement {metric} sample_id",
            )
            require(
                sample_id not in sample_ids,
                f"qualification measurement {metric} duplicated a sample id",
            )
            sample_ids.add(sample_id)
            require(
                sample["repeat"] == sample_index + 1,
                f"qualification measurement {metric} repeat sequence is not exact",
            )
            require(
                sample["matrix_cell_id"] == matrix_cell_id,
                f"qualification measurement {metric} used the wrong host/package matrix cell",
            )
            require(
                sample["workload_id"] == protocol["workloads"][metric]["workload_id"],
                f"qualification measurement {metric} used the wrong workload",
            )
            require(
                sample["cache_state"] == matrix_cell["cache_state"]
                and sample["residency_state"] == matrix_cell["residency_state"],
                f"qualification measurement {metric} changed cache or residency state",
            )
            server_identity = sample["server_identity"]
            require(
                isinstance(server_identity, dict),
                f"qualification measurement {metric} server identity is malformed",
            )
            require_exact_keys(
                server_identity,
                {"server_instance_id", "process_start_id", "load_generation"},
                f"qualification measurement {metric} server identity",
            )
            server_identities.append(
                (
                    require_opaque_identifier(
                        server_identity["server_instance_id"],
                        f"qualification measurement {metric} server_instance_id",
                    ),
                    require_nonempty_string(
                        server_identity["process_start_id"],
                        f"qualification measurement {metric} server process_start_id",
                    ),
                    require_positive_int(
                        server_identity["load_generation"],
                        f"qualification measurement {metric} server load_generation",
                    ),
                )
            )
            sample_values.append(
                qualification_measurement_sample_value(
                    metric,
                    sample,
                    contracts=contracts,
                    phase_boundaries=phase_boundaries,
                    allowed_awake_apis=allowed_awake_apis,
                    inclusive_api=inclusive_api,
                    maximum_suspend_ns=maximum_suspend_ns,
                    expected_policy=expected_policy,
                    expected_backend=expected_backend,
                )
            )
        if sample_policy.get("independence") == "distinct_server_instance_per_sample":
            require(
                len({identity[:2] for identity in server_identities}) == len(samples),
                f"qualification measurement {metric} repeats did not use distinct server instances",
            )
        else:
            require(
                len(set(server_identities)) == 1,
                f"qualification measurement {metric} changed server identity within its repeated block",
            )
        aggregation = sample_policy["aggregation"]
        values[metric] = {
            "maximum": max,
            "minimum": min,
            "exact": lambda raw: raw[0],
        }[aggregation](sample_values)

    require(
        summary["sample_count"] == sample_count,
        "qualification measurement sample count is stale",
    )
    return {
        "artifact": {
            "name": name,
            "sha256": hashlib.sha256(payload_bytes).hexdigest(),
        },
        "values": values,
        "unplanned_suspend": False,
        "matrix_cell_id": matrix_cell_id,
        "payload": payload,
    }


def qualification_measurement_sample_value(
    metric: str,
    sample: dict,
    *,
    contracts: dict,
    phase_boundaries: dict,
    allowed_awake_apis: set[str],
    inclusive_api: str,
    maximum_suspend_ns: int,
    expected_policy: str,
    expected_backend: str,
) -> float | int:
    process = sample["process"]
    require(
        isinstance(process, dict),
        f"qualification measurement {metric} process is malformed",
    )
    require_exact_keys(
        process,
        {"pid", "process_start_id"},
        f"qualification measurement {metric} process",
    )
    require_positive_int(process["pid"], f"qualification measurement {metric} process.pid")
    require_nonempty_string(
        process["process_start_id"],
        f"qualification measurement {metric} process.process_start_id",
    )
    clock = sample["clock"]
    require(
        isinstance(clock, dict),
        f"qualification measurement {metric} clock is malformed",
    )
    require_exact_keys(
        clock,
        {"domain", "api", "boot_id", "resolution_ns"},
        f"qualification measurement {metric} clock",
    )
    require(
        clock["domain"] == "awake_monotonic" and clock["api"] in allowed_awake_apis,
        f"qualification measurement {metric} used an unsupported awake clock",
    )
    boot_id = require_nonempty_string(
        clock["boot_id"],
        f"qualification measurement {metric} clock.boot_id",
    )
    require_nonnegative_int(
        clock["resolution_ns"],
        f"qualification measurement {metric} clock.resolution_ns",
    )
    boundaries = phase_boundaries[metric]
    points: list[tuple[str, int]] = []
    for index, point_name in enumerate(("start", "end")):
        point = sample[point_name]
        require(
            isinstance(point, dict),
            f"qualification measurement {metric} {point_name} is malformed",
        )
        require_exact_keys(
            point,
            {"phase", "observed_ns"},
            f"qualification measurement {metric} {point_name}",
        )
        require(
            point["phase"] == boundaries[index],
            f"qualification measurement {metric} {point_name} phase changed",
        )
        points.append(
            (
                point["phase"],
                require_nonnegative_int(
                    point["observed_ns"],
                    f"qualification measurement {metric} {point_name}.observed_ns",
                ),
            )
        )
    awake_started_ns = points[0][1]
    awake_finished_ns = points[1][1]
    require(
        awake_finished_ns >= awake_started_ns,
        f"qualification measurement {metric} awake clock moved backwards",
    )
    witness = sample["suspend_witness"]
    require(
        isinstance(witness, dict),
        f"qualification measurement {metric} suspend witness is malformed",
    )
    require_exact_keys(
        witness,
        {
            "awake_started_ns",
            "awake_finished_ns",
            "inclusive_clock_api",
            "inclusive_started_ns",
            "inclusive_finished_ns",
            "boot_id_started",
            "boot_id_finished",
        },
        f"qualification measurement {metric} suspend witness",
    )
    require(
        witness["awake_started_ns"] == awake_started_ns
        and witness["awake_finished_ns"] == awake_finished_ns,
        f"qualification measurement {metric} suspend witness changed phase timestamps",
    )
    require(
        witness["inclusive_clock_api"] == inclusive_api,
        f"qualification measurement {metric} used the wrong suspend-inclusive clock",
    )
    inclusive_started_ns = require_nonnegative_int(
        witness["inclusive_started_ns"],
        f"qualification measurement {metric} suspend witness inclusive_started_ns",
    )
    inclusive_finished_ns = require_nonnegative_int(
        witness["inclusive_finished_ns"],
        f"qualification measurement {metric} suspend witness inclusive_finished_ns",
    )
    require(
        inclusive_finished_ns >= inclusive_started_ns,
        f"qualification measurement {metric} suspend-inclusive clock moved backwards",
    )
    require(
        witness["boot_id_started"] == boot_id
        and witness["boot_id_finished"] == boot_id,
        f"qualification measurement {metric} crossed a boot boundary",
    )
    awake_delta_ns = awake_finished_ns - awake_started_ns
    inclusive_delta_ns = inclusive_finished_ns - inclusive_started_ns
    require(
        abs(inclusive_delta_ns - awake_delta_ns) <= maximum_suspend_ns,
        f"qualification measurement {metric} crossed an unplanned suspend or power transition",
    )

    operands = sample["operands"]
    require(
        isinstance(operands, dict),
        f"qualification measurement {metric} operands are malformed",
    )
    duration_metrics = {
        "existing_owner_connect",
        "spawn_convergence",
        "cold_first_vector",
        "first_product_ready",
        "warm_query_ipc",
        "warm_bulk_ipc",
        "busy_retry_usefulness",
        "true_idle_exit",
    }
    successful_operation_metrics = {
        "cold_first_vector",
        "first_product_ready",
        "warm_query_ipc",
        "warm_bulk_ipc",
        "bulk_documents_per_second",
        "bulk_tokens_per_second",
    }
    if metric in duration_metrics:
        require_exact_keys(
            operands,
            (
                {"successful_operation_duration_ns"}
                if metric in successful_operation_metrics
                else set()
            ),
            f"qualification measurement {metric} operands",
        )
        if metric in successful_operation_metrics:
            operation_duration_ns = require_nonnegative_int(
                operands["successful_operation_duration_ns"],
                f"qualification measurement {metric} successful operation duration",
            )
            require(
                operation_duration_ns == awake_delta_ns,
                f"qualification measurement {metric} successful operation duration differs from its awake interval",
            )
        return awake_delta_ns / 1_000_000
    if metric in {"bulk_documents_per_second", "bulk_tokens_per_second"}:
        operand = (
            "completed_documents"
            if metric == "bulk_documents_per_second"
            else "completed_tokens"
        )
        require_exact_keys(
            operands,
            {operand, "successful_operation_duration_ns"},
            f"qualification measurement {metric} operands",
        )
        completed = require_positive_int(
            operands[operand],
            f"qualification measurement {metric} operands.{operand}",
        )
        require(
            awake_delta_ns > 0,
            f"qualification measurement {metric} window is empty",
        )
        require(
            require_nonnegative_int(
                operands["successful_operation_duration_ns"],
                f"qualification measurement {metric} successful operation duration",
            )
            == awake_delta_ns,
            f"qualification measurement {metric} successful operation duration differs from its awake interval",
        )
        return completed * 1_000_000_000 / awake_delta_ns
    if metric == "total_codestory_process_memory":
        require_exact_keys(
            operands,
            {"processes"},
            "qualification five-process memory operands",
        )
        processes = operands["processes"]
        require(
            isinstance(processes, list) and len(processes) == 5,
            "qualification memory evidence must contain exactly five processes",
        )
        expected_roles = {
            "plugin_host_a",
            "plugin_cli_a",
            "plugin_host_b",
            "plugin_cli_b",
            "embedding_server",
        }
        identities: set[tuple[int, str]] = set()
        roles: set[str] = set()
        total = 0
        for index, observed in enumerate(processes):
            require(
                isinstance(observed, dict),
                f"qualification memory process {index} is malformed",
            )
            require_exact_keys(
                observed,
                {
                    "role",
                    "pid",
                    "process_start_id",
                    "executable_sha256",
                    "resident_bytes",
                    "measurement_api",
                },
                f"qualification memory process {index}",
            )
            roles.add(
                require_nonempty_string(
                    observed["role"],
                    f"qualification memory process {index}.role",
                )
            )
            pid = require_positive_int(
                observed["pid"],
                f"qualification memory process {index}.pid",
            )
            start_id = require_nonempty_string(
                observed["process_start_id"],
                f"qualification memory process {index}.process_start_id",
            )
            identities.add((pid, start_id))
            require_sha256(
                observed["executable_sha256"],
                f"qualification memory process {index}.executable_sha256",
            )
            total += require_positive_int(
                observed["resident_bytes"],
                f"qualification memory process {index}.resident_bytes",
            )
            require_nonempty_string(
                observed["measurement_api"],
                f"qualification memory process {index}.measurement_api",
            )
        require(
            roles == expected_roles and len(identities) == 5,
            "qualification memory evidence changed the five-process set",
        )
        return total
    if metric == "retrieval_quality":
        require_exact_keys(
            operands,
            {"publishable_packet_pass", "raw_artifact_sha256"},
            "qualification retrieval quality operands",
        )
        require_sha256(
            operands["raw_artifact_sha256"],
            "qualification retrieval quality raw artifact sha256",
        )
        require(
            operands["publishable_packet_pass"] is True,
            "qualification retrieval quality sample did not pass",
        )
        return 1
    require(
        metric == "backend_observed_accelerator_residency",
        f"qualification measurement verifier omitted metric {metric}",
    )
    require_exact_keys(
        operands,
        {
            "policy",
            "backend",
            "accelerator_execution_verified",
            "resident_accelerator_tensor_count",
            "resident_accelerator_tensor_bytes",
            "offloaded_layer_count",
            "model_layer_count",
        },
        f"qualification measurement {metric} operands",
    )
    require(
        operands["policy"] == expected_policy
        and normalized_backend(operands["backend"])
        == normalized_backend(expected_backend),
        "qualification accelerator-residency sample used the wrong policy or backend",
    )
    tensor_count = require_nonnegative_int(
        operands["resident_accelerator_tensor_count"],
        "qualification accelerator resident tensor count",
    )
    tensor_bytes = require_nonnegative_int(
        operands["resident_accelerator_tensor_bytes"],
        "qualification accelerator resident tensor bytes",
    )
    offloaded_layers = require_nonnegative_int(
        operands["offloaded_layer_count"],
        "qualification accelerator offloaded layer count",
    )
    model_layers = require_positive_int(
        operands["model_layer_count"],
        "qualification accelerator model layer count",
    )
    policy_residency_valid = (
        expected_policy == "accelerated"
        and operands["accelerator_execution_verified"] is True
        and tensor_count > 0
        and tensor_bytes > 0
        and offloaded_layers == model_layers
    ) or (
        expected_policy == "cpu_explicit"
        and operands["accelerator_execution_verified"] is False
        and tensor_count == 0
        and tensor_bytes == 0
        and offloaded_layers == 0
    )
    require(
        policy_residency_valid,
        "qualification accelerator-residency operands contradict the selected policy",
    )
    return 1


def verify_calibration_source_lineage(
    calibration_source: dict,
    frozen_source: dict,
    repository_root: Path,
) -> dict:
    require(
        frozen_source.get("tracked_dirty") is False,
        "frozen package source tree was dirty",
    )
    for label, source in (
        ("calibration", calibration_source),
        ("frozen package", frozen_source),
    ):
        require(
            isinstance(source.get("commit"), str)
            and re.fullmatch(r"[0-9a-f]{40}", source["commit"]) is not None
            and isinstance(source.get("tree"), str)
            and re.fullmatch(r"[0-9a-f]{40}", source["tree"]) is not None,
            f"{label} source identity is not an exact Git commit and tree",
        )
    require(
        calibration_source["commit"] != frozen_source["commit"],
        "frozen package did not add the required constant-set freeze commit",
    )

    def git(*arguments: str) -> str:
        completed = subprocess.run(
            ["git", *arguments],
            cwd=repository_root,
            text=True,
            capture_output=True,
            timeout=30,
        )
        require(
            completed.returncode == 0,
            "calibration source-lineage probe failed: "
            + require_nonempty_string(
                completed.stderr.strip() or completed.stdout.strip(),
                "Git lineage failure",
            ),
        )
        return completed.stdout.strip()

    require(
        git("rev-parse", "HEAD") == frozen_source["commit"]
        and git("rev-parse", "HEAD^{tree}") == frozen_source["tree"],
        "verification checkout does not match the frozen package source",
    )
    require(
        git("rev-parse", f"{calibration_source['commit']}^{{tree}}")
        == calibration_source["tree"],
        "calibration commit does not resolve to the recorded calibration tree",
    )
    completed = subprocess.run(
        [
            "git",
            "merge-base",
            "--is-ancestor",
            calibration_source["commit"],
            frozen_source["commit"],
        ],
        cwd=repository_root,
        capture_output=True,
        timeout=30,
    )
    require(
        completed.returncode == 0,
        "calibration source is not an ancestor of the frozen package source",
    )
    changed_paths = [
        path
        for path in git(
            "diff",
            "--name-only",
            calibration_source["commit"],
            frozen_source["commit"],
        ).splitlines()
        if path
    ]
    require(
        changed_paths
        == ["docs/testing/per-user-embedding-server-constant-set.json"],
        "post-calibration source drift exceeded the one allowed constant-set freeze file",
    )
    return {
        "selection_commit": calibration_source["commit"],
        "frozen_commit": frozen_source["commit"],
        "allowed_changed_paths": changed_paths,
    }


def verify_calibration_bundle(
    path: Path,
    measurement_contract: dict,
    *,
    compare_frozen_constant_set: bool = True,
    frozen_source: dict | None = None,
    repository_root: Path | None = None,
    enforce_source_lineage: bool = False,
    expected_producer_run_id: str | None = None,
    expected_producer_artifact: str | None = None,
) -> dict:
    require(path.is_file() and not path.is_symlink(), f"calibration bundle is missing or unsafe: {path}")
    try:
        bundle = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"calibration bundle is not valid JSON: {exc}") from exc
    require(isinstance(bundle, dict), "calibration bundle must be an object")
    require_exact_keys(
        bundle,
        {
            "schema_version",
            "selection_protocol",
            "source",
            "producer",
            "contracts",
            "runs",
            "freeze_digest",
        },
        "calibration bundle",
    )
    require(bundle["schema_version"] == 1, "calibration bundle schema is unsupported")
    constant_set = measurement_contract["constant_set"]
    protocol = measurement_contract["measurement_protocol"]
    require(
        bundle["selection_protocol"] == constant_set["selection_protocol"],
        "calibration bundle used a different selection protocol",
    )
    source = bundle["source"]
    require(isinstance(source, dict), "calibration bundle source identity is malformed")
    require_exact_keys(source, {"commit", "tree", "tracked_dirty"}, "calibration bundle source")
    for field in ("commit", "tree"):
        require(
            isinstance(source[field], str)
            and re.fullmatch(r"[0-9a-f]{40}", source[field]) is not None,
            f"calibration bundle source.{field} is not an exact Git object id",
        )
    require(source["tracked_dirty"] is False, "calibration bundle source tree was dirty")
    producer = bundle["producer"]
    require(isinstance(producer, dict), "calibration bundle producer is malformed")
    require_exact_keys(
        producer,
        {
            "repository",
            "workflow_path",
            "run_id",
            "run_attempt",
            "artifact_name",
            "source_head_sha",
        },
        "calibration bundle producer",
    )
    require(
        producer["repository"] == "TheGreenCedar/CodeStory"
        and producer["workflow_path"]
        == ".github/workflows/packaged-platform-pr.yml"
        and isinstance(producer["run_id"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
        and isinstance(producer["run_attempt"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"]) is not None
        and producer["artifact_name"]
        == f"embedding-calibration-bundle-{source['commit']}"
        and producer["source_head_sha"] == source["commit"],
        "calibration bundle producer is not the trusted exact-head coordinator artifact",
    )
    if expected_producer_run_id is not None or expected_producer_artifact is not None:
        require(
            producer["run_id"] == expected_producer_run_id
            and producer["artifact_name"] == expected_producer_artifact,
            "calibration bundle producer differs from the authenticated download request",
        )
    contracts = bundle["contracts"]
    expected_contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "measurement_protocol_sha256": measurement_contract[
            "measurement_protocol_sha256"
        ],
        "input_constant_set_sha256": (
            constant_set["freeze_record"]["input_constant_set_sha256"]
            if constant_set.get("status") == "frozen"
            and isinstance(constant_set.get("freeze_record"), dict)
            else measurement_contract["constant_set_sha256"]
        ),
    }
    require(
        contracts == expected_contracts,
        "calibration bundle contract hashes differ from the checked-in protocols",
    )
    runs = bundle["runs"]
    matrix = protocol["calibration_matrix"]
    expected_run_cells = {
        (matrix_cell_id, run_index)
        for matrix_cell_id in matrix
        for run_index in range(1, 4)
    }
    require(
        isinstance(runs, list) and len(runs) == len(expected_run_cells),
        "calibration bundle must contain exactly three runs for every matrix cell",
    )
    observed_run_cells: set[tuple[str, int]] = set()
    run_ids: set[str] = set()
    artifact_digests: set[str] = set()
    packages_by_cell: dict[str, dict] = {}
    global_sample_ids: set[str] = set()
    calibration_metrics = set(protocol["required_metrics"]) - {"retrieval_quality"}
    run_metric_values: dict[str, list[float | int]] = {
        metric: [] for metric in calibration_metrics
    }
    raw_duration_values_ms: dict[str, list[float]] = {
        "existing_owner_connect_duration": [],
        "spawn_convergence_duration": [],
        "query_request_duration": [],
        "bulk_request_duration": [],
        "capacity_condition_duration": [],
        "successful_operation_duration": [],
    }
    phase_boundaries = protocol["phase_boundaries"]
    metric_contracts = protocol["metric_contracts"]
    sampling = protocol["metric_sampling"]
    suspend_contract = protocol["clock_policy"]["suspend_detection"]
    maximum_suspend_ns = require_nonnegative_int(
        suspend_contract["maximum_inclusive_minus_awake_ns"],
        "calibration suspend-detection tolerance",
    )
    for run_position, run in enumerate(runs):
        require(isinstance(run, dict), f"calibration run {run_position} is malformed")
        require_exact_keys(
            run,
            {
                "run_id_sha256",
                "matrix_cell_id",
                "run_index",
                "host_fingerprint",
                "clean",
                "unplanned_suspend",
                "source",
                "contracts",
                "package",
                "raw_artifact",
            },
            f"calibration run {run_position}",
        )
        run_id = require_sha256(
            run["run_id_sha256"],
            f"calibration run {run_position}.run_id_sha256",
        )
        require(run_id not in run_ids, "calibration bundle duplicated an independent run id")
        run_ids.add(run_id)
        matrix_cell_id = require_nonempty_string(
            run["matrix_cell_id"],
            f"calibration run {run_position}.matrix_cell_id",
        )
        run_index = require_positive_int(
            run["run_index"],
            f"calibration run {run_position}.run_index",
        )
        run_cell = (matrix_cell_id, run_index)
        require(
            run_cell in expected_run_cells and run_cell not in observed_run_cells,
            f"calibration run {run_position} duplicated or escaped the exact matrix",
        )
        observed_run_cells.add(run_cell)
        host_fingerprint = require_sha256(
            run["host_fingerprint"],
            f"calibration run {run_position}.host_fingerprint",
        )
        require(
            run["clean"] is True and run["unplanned_suspend"] is False,
            f"calibration run {run_position} was not a clean awake run",
        )
        require(
            run["source"] == source and run["contracts"] == contracts,
            f"calibration run {run_position} changed source, tree, or protocol identity",
        )
        matrix_cell = matrix[matrix_cell_id]
        package = run["package"]
        require(isinstance(package, dict), f"calibration run {run_position} package is malformed")
        require_exact_keys(
            package,
            {
                "archive_sha256",
                "executable_sha256",
                "asset_target",
                "release_version",
                "model_sha256",
                "policy",
                "backend",
            },
            f"calibration run {run_position} package",
        )
        for field in ("archive_sha256", "executable_sha256", "model_sha256"):
            require_sha256(package[field], f"calibration run {run_position} package.{field}")
        require(
            package["asset_target"] == matrix_cell["asset_target"]
            and package["policy"] == matrix_cell["policy"]
            and normalized_backend(package["backend"])
            == normalized_backend(matrix_cell["backend"])
            and isinstance(package["release_version"], str)
            and bool(package["release_version"]),
            f"calibration run {run_position} package does not match its matrix cell",
        )
        if matrix_cell_id in packages_by_cell:
            require(
                package == packages_by_cell[matrix_cell_id],
                f"calibration matrix cell {matrix_cell_id} changed package between runs",
            )
        else:
            packages_by_cell[matrix_cell_id] = package
        raw_artifact = run["raw_artifact"]
        require(isinstance(raw_artifact, dict), f"calibration run {run_position} raw artifact is malformed")
        require_exact_keys(
            raw_artifact,
            {"name", "sha256", "payload"},
            f"calibration run {run_position} raw artifact",
        )
        require(
            raw_artifact["name"] == "measurements.raw.json",
            f"calibration run {run_position} raw artifact has the wrong name",
        )
        artifact_digest = require_sha256(
            raw_artifact["sha256"],
            f"calibration run {run_position} raw artifact sha256",
        )
        require(
            artifact_digest == canonical_sha256(raw_artifact["payload"]),
            f"calibration run {run_position} raw artifact digest does not match its payload",
        )
        require(
            artifact_digest not in artifact_digests,
            "calibration bundle reused one raw artifact for multiple independent runs",
        )
        artifact_digests.add(artifact_digest)
        raw = raw_artifact["payload"]
        require(isinstance(raw, dict), f"calibration run {run_position} raw payload is malformed")
        require_exact_keys(
            raw,
            {
                "schema_version",
                "run_id_sha256",
                "matrix_cell_id",
                "run_index",
                "host_fingerprint",
                "source",
                "contracts",
                "package",
                "clean",
                "unplanned_suspend",
                "metrics",
            },
            f"calibration run {run_position} raw payload",
        )
        require(
            raw["schema_version"] == 1
            and raw["run_id_sha256"] == run_id
            and raw["matrix_cell_id"] == matrix_cell_id
            and raw["run_index"] == run_index
            and raw["host_fingerprint"] == host_fingerprint
            and raw["source"] == source
            and raw["contracts"] == contracts
            and raw["package"] == package
            and raw["clean"] is True
            and raw["unplanned_suspend"] is False,
            f"calibration run {run_position} raw payload identity is stale",
        )
        metrics = raw["metrics"]
        require(
            isinstance(metrics, dict)
            and set(metrics) == calibration_metrics,
            f"calibration run {run_position} omitted a required metric",
        )
        target_os = TARGET_CONTRACTS[matrix_cell["asset_target"]]["target_os"]
        allowed_awake_apis = set(protocol["clock_policy"]["platform_apis"][target_os])
        inclusive_api = suspend_contract["platform_apis"][target_os]
        for metric in sorted(metrics):
            record = metrics[metric]
            require(isinstance(record, dict), f"calibration run {run_position} metric {metric} is malformed")
            require_exact_keys(
                record,
                {"unit", "samples"},
                f"calibration run {run_position} metric {metric}",
            )
            require(
                record["unit"] == metric_contracts[metric]["unit"],
                f"calibration run {run_position} metric {metric} used the wrong unit",
            )
            samples = record["samples"]
            require(
                isinstance(samples, list)
                and len(samples) == sampling[metric]["sample_count"],
                f"calibration run {run_position} metric {metric} sample count changed",
            )
            values = []
            server_identities = []
            for sample_index, sample in enumerate(samples):
                require(isinstance(sample, dict), f"calibration {metric} sample is malformed")
                require_exact_keys(
                    sample,
                    {
                        "sample_id",
                        "repeat",
                        "matrix_cell_id",
                        "workload_id",
                        "cache_state",
                        "residency_state",
                        "process",
                        "server_identity",
                        "clock",
                        "start",
                        "end",
                        "operands",
                        "suspend_witness",
                    },
                    f"calibration run {run_position} metric {metric} sample {sample_index}",
                )
                sample_id = require_opaque_identifier(
                    sample["sample_id"],
                    f"calibration run {run_position} metric {metric} sample id",
                )
                require(
                    sample_id not in global_sample_ids,
                    "calibration bundle duplicated a sample identity",
                )
                global_sample_ids.add(sample_id)
                require(
                    sample["repeat"] == sample_index + 1
                    and sample["matrix_cell_id"] == matrix_cell_id
                    and sample["workload_id"] == protocol["workloads"][metric]["workload_id"]
                    and sample["cache_state"] == matrix_cell["cache_state"]
                    and sample["residency_state"] == matrix_cell["residency_state"],
                    f"calibration run {run_position} metric {metric} sample identity changed",
                )
                server_identity = sample["server_identity"]
                require(
                    isinstance(server_identity, dict),
                    f"calibration run {run_position} metric {metric} server identity is malformed",
                )
                require_exact_keys(
                    server_identity,
                    {"server_instance_id", "process_start_id", "load_generation"},
                    f"calibration run {run_position} metric {metric} server identity",
                )
                server_identities.append(
                    (
                        require_opaque_identifier(
                            server_identity["server_instance_id"],
                            f"calibration run {run_position} metric {metric} server_instance_id",
                        ),
                        require_nonempty_string(
                            server_identity["process_start_id"],
                            f"calibration run {run_position} metric {metric} process_start_id",
                        ),
                        require_positive_int(
                            server_identity["load_generation"],
                            f"calibration run {run_position} metric {metric} load_generation",
                        ),
                    )
                )
                value = qualification_measurement_sample_value(
                    metric,
                    sample,
                    contracts=contracts,
                    phase_boundaries=phase_boundaries,
                    allowed_awake_apis=allowed_awake_apis,
                    inclusive_api=inclusive_api,
                    maximum_suspend_ns=maximum_suspend_ns,
                    expected_policy=matrix_cell["policy"],
                    expected_backend=matrix_cell["backend"],
                )
                values.append(value)
                awake_delta_ms = (
                    sample["end"]["observed_ns"] - sample["start"]["observed_ns"]
                ) / 1_000_000
                if metric == "existing_owner_connect":
                    raw_duration_values_ms["existing_owner_connect_duration"].append(
                        awake_delta_ms
                    )
                if metric == "spawn_convergence":
                    raw_duration_values_ms["spawn_convergence_duration"].append(
                        awake_delta_ms
                    )
                if metric in {"cold_first_vector", "first_product_ready", "warm_query_ipc"}:
                    raw_duration_values_ms["query_request_duration"].append(awake_delta_ms)
                if metric in {
                    "warm_bulk_ipc",
                    "bulk_documents_per_second",
                    "bulk_tokens_per_second",
                }:
                    raw_duration_values_ms["bulk_request_duration"].append(awake_delta_ms)
                if metric == "busy_retry_usefulness":
                    raw_duration_values_ms["capacity_condition_duration"].append(
                        awake_delta_ms
                    )
                if metric in {
                    "cold_first_vector",
                    "first_product_ready",
                    "warm_query_ipc",
                    "warm_bulk_ipc",
                    "bulk_documents_per_second",
                    "bulk_tokens_per_second",
                }:
                    raw_duration_values_ms["successful_operation_duration"].append(
                        require_nonnegative_int(
                            sample["operands"]["successful_operation_duration_ns"],
                            f"calibration run {run_position} metric {metric} successful operation duration",
                        )
                        / 1_000_000
                    )
            if sampling[metric].get("independence") == "distinct_server_instance_per_sample":
                require(
                    len({identity[:2] for identity in server_identities}) == len(samples),
                    f"calibration run {run_position} metric {metric} samples are not independent",
                )
            else:
                require(
                    len(set(server_identities)) == 1,
                    f"calibration run {run_position} metric {metric} changed server identity",
                )
            aggregation = sampling[metric]["aggregation"]
            run_metric_values[metric].append(
                {
                    "maximum": max,
                    "minimum": min,
                    "exact": lambda raw_values: raw_values[0],
                    "all_rows_pass_rate": lambda raw_values: sum(raw_values)
                    / len(raw_values),
                }[aggregation](values)
            )
    require(
        observed_run_cells == expected_run_cells,
        "calibration bundle does not exactly cover every matrix cell three times",
    )
    require(
        len({package["release_version"] for package in packages_by_cell.values()}) == 1
        and len({package["model_sha256"] for package in packages_by_cell.values()}) == 1,
        "calibration matrix cells did not use one release version and model",
    )
    require(
        all(raw_duration_values_ms.values()),
        "calibration bundle omitted a production-constant raw source cell",
    )
    connect_timeout_ms = max(
        1, math.ceil(max(raw_duration_values_ms["existing_owner_connect_duration"]) * 1.50)
    )
    spawn_timeout_ms = max(
        1, math.ceil(max(raw_duration_values_ms["spawn_convergence_duration"]) * 1.50)
    )
    query_deadline_ms = max(
        1, math.ceil(max(raw_duration_values_ms["query_request_duration"]) * 1.50)
    )
    bulk_replay_success_budget_ms = max(
        query_deadline_ms,
        math.ceil(max(raw_duration_values_ms["bulk_request_duration"]) * 1.50),
    )
    retry_after_ms = max(
        1, math.floor(min(raw_duration_values_ms["capacity_condition_duration"]) * 0.50)
    )
    initial_backoff_ms = max(
        1,
        math.ceil(
            max(raw_duration_values_ms["existing_owner_connect_duration"]) * 0.50
        ),
    )
    maximum_backoff_ms = max(
        initial_backoff_ms,
        math.ceil(max(raw_duration_values_ms["spawn_convergence_duration"]) * 0.25),
    )
    hard_no_progress_ms = max(
        1,
        math.ceil(max(raw_duration_values_ms["successful_operation_duration"]) * 4.00),
    )
    watchdog_cadence_ms = max(1, math.floor(hard_no_progress_ms / 20))
    bulk_deadline_ms = (
        hard_no_progress_ms
        + watchdog_cadence_ms
        + spawn_timeout_ms
        + bulk_replay_success_budget_ms
    )
    selected_constants = {
        "connect_timeout_ms": connect_timeout_ms,
        "spawn_convergence_timeout_ms": spawn_timeout_ms,
        "request_deadlines_ms": {
            "query_request_deadline_ms": query_deadline_ms,
            "bulk_replay_success_budget_ms": bulk_replay_success_budget_ms,
            "bulk_request_deadline_ms": bulk_deadline_ms,
        },
        "capacity_retry_policy": {
            "retry_after_ms": retry_after_ms,
            "retry_class": "after_capacity_change",
            "retry_condition_source": "named_condition_from_typed_capacity_response",
        },
        "election_backoff_policy": {
            "initial_backoff_ms": initial_backoff_ms,
            "maximum_backoff_ms": maximum_backoff_ms,
            "jitter": (
                "sha256(process_start_id||attempt) modulo inclusive "
                "[initial_backoff_ms,maximum_backoff_ms]"
            ),
        },
        "hard_native_no_progress_ms": hard_no_progress_ms,
        "watchdog_cadence_ms": watchdog_cadence_ms,
    }
    thresholds: dict[str, float | int] = {}
    for metric, values in run_metric_values.items():
        comparison = metric_contracts[metric]["comparison"]
        if metric == "retrieval_quality":
            threshold: float | int = 1.0
        elif comparison == "less_than_or_equal":
            threshold = math.ceil(max(values) * 1.20)
        elif comparison == "greater_than_or_equal":
            threshold = math.floor(min(values) * 0.80)
        else:
            require(
                len(set(values)) == 1,
                f"calibration equal metric {metric} did not have one exact observed value",
            )
            threshold = values[0]
        thresholds[metric] = threshold
    thresholds["retrieval_quality"] = 1.0
    if compare_frozen_constant_set:
        require(
            constant_set["calibration_required_values"] == selected_constants,
            "frozen compiled constants do not match the preregistered calibration formulas",
        )
        require(
            constant_set["qualification_thresholds"] == thresholds,
            "frozen qualification thresholds do not match the preregistered calibration formulas",
        )
    freeze_payload = {
        "selection_protocol": bundle["selection_protocol"],
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "run_artifact_sha256s": sorted(artifact_digests),
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
    }
    freeze_digest = canonical_sha256(freeze_payload)
    source_lineage = None
    if compare_frozen_constant_set and enforce_source_lineage:
        require(
            frozen_source is not None and repository_root is not None,
            "frozen qualification requires exact calibration-to-package source lineage",
        )
        source_lineage = verify_calibration_source_lineage(
            source,
            frozen_source,
            repository_root,
        )
    if compare_frozen_constant_set:
        require(
            bundle["freeze_digest"] == freeze_digest,
            "calibration bundle freeze digest does not match recomputed raw evidence",
        )
        freeze_record = constant_set["freeze_record"]
        require(
            freeze_record["selection_source_commit"] == source["commit"]
            and freeze_record["selection_source_tree"] == source["tree"]
            and freeze_record["measurement_protocol_sha256"]
            == contracts["measurement_protocol_sha256"]
            and freeze_record["protocol_sha256"] == contracts["protocol_sha256"]
            and freeze_record["input_constant_set_sha256"]
            == contracts["input_constant_set_sha256"]
            and freeze_record["calibration_bundle_sha256"] == sha256(path)
            and freeze_record["calibration_freeze_digest"] == freeze_digest
            and sorted(freeze_record["run_artifact_sha256s"])
            == sorted(artifact_digests)
            and freeze_record["selection_rule"]
            == "all_preregistered_clean_runs_no_outlier_removal",
            "constant-set freeze record does not bind the exact recomputed calibration bundle",
        )
    return {
        "artifact": {"name": path.name, "sha256": sha256(path)},
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "matrix_cell_count": len(matrix),
        "run_count": len(runs),
        "freeze_digest": freeze_digest,
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
        "run_artifact_sha256s": sorted(artifact_digests),
        "source_lineage": source_lineage,
    }


def assemble_calibration_bundle(args: argparse.Namespace) -> dict:
    require(
        args.calibration_bundle_output is not None
        and args.frozen_constant_set_output is not None
        and args.freeze_selected_at is not None,
        "calibration assembly requires bundle output, frozen constant-set output, and selected-at",
    )
    measurement_contract = load_server_measurement_contract(
        args.measurement_protocol
    )
    constant_set = measurement_contract["constant_set"]
    require(
        constant_set.get("status") == "unfrozen"
        and constant_set.get("freeze_record") is None,
        "calibration assembly requires the exact unfrozen input constant set",
    )
    expected_run_count = (
        len(measurement_contract["measurement_protocol"]["calibration_matrix"]) * 3
    )
    run_paths = [path.resolve() for path in args.calibration_run]
    require(
        len(run_paths) == expected_run_count
        and len(set(run_paths)) == expected_run_count,
        f"calibration assembly requires exactly {expected_run_count} distinct run artifacts",
    )
    runs = []
    for position, path in enumerate(run_paths):
        require(
            path.is_file() and not path.is_symlink(),
            f"calibration run artifact {position} is missing or unsafe: {path}",
        )
        try:
            run = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise ProofFailure(
                f"calibration run artifact {position} is invalid JSON: {exc}"
            ) from exc
        require(
            isinstance(run, dict),
            f"calibration run artifact {position} must be an object",
        )
        runs.append(run)
    first = runs[0]
    source = first.get("source")
    contracts = first.get("contracts")
    require(
        isinstance(source, dict) and isinstance(contracts, dict),
        "first calibration run omitted source or contract identity",
    )
    producer = {
        "repository": args.calibration_producer_repository,
        "workflow_path": args.calibration_producer_workflow_path,
        "run_id": args.calibration_producer_run_id,
        "run_attempt": args.calibration_producer_run_attempt,
        "artifact_name": args.calibration_producer_artifact,
        "source_head_sha": source.get("commit"),
    }
    bundle = {
        "schema_version": 1,
        "selection_protocol": constant_set["selection_protocol"],
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "runs": runs,
        "freeze_digest": "",
    }
    bundle_output = args.calibration_bundle_output.resolve()
    constant_output = args.frozen_constant_set_output.resolve()
    require(
        bundle_output != constant_output,
        "calibration bundle and frozen constant-set outputs must be distinct",
    )
    write_json(bundle_output, bundle)
    selection = verify_calibration_bundle(
        bundle_output,
        measurement_contract,
        compare_frozen_constant_set=False,
    )
    bundle["freeze_digest"] = selection["freeze_digest"]
    write_json(bundle_output, bundle)

    frozen_constant_set = json.loads(json.dumps(constant_set))
    frozen_constant_set["status"] = "frozen"
    frozen_constant_set["calibration_required_values"] = selection[
        "calibration_required_values"
    ]
    frozen_constant_set["qualification_thresholds"] = selection[
        "qualification_thresholds"
    ]
    frozen_constant_set["freeze_record"] = {
        "selection_source_commit": source["commit"],
        "selection_source_tree": source["tree"],
        "measurement_protocol_sha256": contracts["measurement_protocol_sha256"],
        "protocol_sha256": contracts["protocol_sha256"],
        "input_constant_set_sha256": contracts["input_constant_set_sha256"],
        "calibration_bundle_sha256": sha256(bundle_output),
        "calibration_freeze_digest": selection["freeze_digest"],
        "run_artifact_sha256s": selection["run_artifact_sha256s"],
        "selection_rule": "all_preregistered_clean_runs_no_outlier_removal",
        "selected_at": require_nonempty_string(
            args.freeze_selected_at,
            "--freeze-selected-at",
        ),
    }
    write_json(constant_output, frozen_constant_set)
    frozen_contract = json.loads(json.dumps(measurement_contract))
    frozen_contract["constant_set"] = frozen_constant_set
    frozen_contract["constant_set_sha256"] = sha256(constant_output)
    verified = verify_calibration_bundle(
        bundle_output,
        frozen_contract,
        enforce_source_lineage=False,
    )
    return {
        "bundle": verified["artifact"],
        "frozen_constant_set": {
            "name": constant_output.name,
            "sha256": sha256(constant_output),
        },
        "selection_source": source,
        "run_count": verified["run_count"],
        "matrix_cell_count": verified["matrix_cell_count"],
        "freeze_digest": verified["freeze_digest"],
    }


def produce_qualification_evidence(
    args: argparse.Namespace,
    qualification_cli: Path,
    env: dict[str, str],
    root: Path,
    runtime: dict,
    manifest: dict,
    archive_sha256: str,
    measurement_contract: dict,
) -> dict:
    require(
        args.qualification_evidence is not None,
        "--produce-qualification-evidence requires --qualification-evidence",
    )
    require(
        qualification_cli.is_file(),
        f"qualification executable is missing: {qualification_cli}",
    )
    require(
        sha256(qualification_cli) == manifest["binary"]["sha256"],
        "qualification executable does not match the packaged executable",
    )
    private_root = root / "qualification-suite"
    artifact_root = private_root / "artifacts"
    private_root.mkdir(mode=0o700)
    artifact_root.mkdir(mode=0o700)
    nonce = secrets.token_hex(32)
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    projects = runtime.get("_qualification_projects")
    require(
        isinstance(projects, list)
        and len(projects) == 2
        and all(isinstance(project, str) and Path(project).is_absolute() for project in projects),
        "runtime proof omitted its two qualification projects",
    )
    contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "constant_set_sha256": measurement_contract["constant_set_sha256"],
        "measurement_protocol_sha256": measurement_contract["measurement_protocol_sha256"],
    }
    package = {
        "archive_sha256": archive_sha256,
        "executable_sha256": manifest["binary"]["sha256"],
        "asset_target": manifest["asset_target"],
        "release_version": manifest["release_version"],
    }
    qualification_env = dict(env)
    qualification_env.pop("CODESTORY_CLI", None)
    qualification_env["CODESTORY_EMBED_QUALIFICATION_DIR"] = str(private_root.resolve())
    qualification_env["CODESTORY_EMBED_QUALIFICATION_NONCE"] = nonce
    qualification_env["CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"] = archive_sha256
    fault_recovery_consistency_evidence = None
    if args.publication_fault_evidence is None:
        (
            args.publication_fault_evidence,
            consistency_path,
        ) = produce_product_publication_fault_evidence(
            qualification_cli,
            qualification_env,
            private_root,
            artifact_root,
            nonce,
            source=manifest["source"],
            package=package,
            contracts=contracts,
            timeout=args.timeout_secs,
        )
        fault_recovery_consistency_evidence = (
            verify_fault_recovery_consistency_raw_evidence(
                consistency_path,
                source=manifest["source"],
                package=package,
                contracts=contracts,
            )
        )
    publication_fault_evidence = verify_publication_fault_raw_evidence(
        args.publication_fault_evidence,
        source=manifest["source"],
        package=package,
        contracts=contracts,
    )
    retrieval_quality_evidence = None
    if args.retrieval_quality_evidence is not None:
        retrieval_quality_evidence = verify_retrieval_quality_raw_evidence(
            args.retrieval_quality_evidence,
            source=manifest["source"],
        )
    elif args.proof_tier != "calibration":
        raise ProofFailure(
            f"{args.proof_tier} qualification requires --retrieval-quality-evidence "
            f"from {RETRIEVAL_QUALITY_EVIDENCE_CONTRACT}"
        )
    identity = runtime["identity"]
    expected_backend = args.expected_backend or require_nonempty_string(
        identity.get("embedding_backend"),
        "runtime embedding backend",
    )
    matrix_cell_id = require_nonempty_string(
        args.qualification_matrix_cell,
        "--produce-qualification-evidence requires --qualification-matrix-cell",
    )
    matrix_cell = selected_qualification_matrix_cell(
        measurement_contract["measurement_protocol"],
        cell_id=matrix_cell_id,
        target=manifest["asset_target"],
        proof_tier=args.proof_tier,
        expected_policy=args.engine_policy,
        expected_backend=expected_backend,
    )
    request = {
        "schema_version": 1,
        "qualification_nonce": nonce,
        "qualification_nonce_sha256": nonce_sha256,
        "proof_tier": args.proof_tier,
        "source": manifest["source"],
        "package": package,
        "contracts": contracts,
        "runtime": {
            "engine_policy": args.engine_policy,
            "expected_backend": expected_backend,
            "offline": args.offline,
            "matrix_cell_id": matrix_cell_id,
            "cache_state": matrix_cell["cache_state"],
            "residency_state": matrix_cell["residency_state"],
        },
        "projects": projects,
        "required_scenarios": sorted(REQUIRED_SERVER_SCENARIOS),
        "required_metrics": sorted(
            measurement_contract["measurement_protocol"]["required_metrics"]
        ),
        "output_directory": str(artifact_root.resolve()),
    }
    qualification_env["CODESTORY_EMBED_QUALIFICATION_DIR"] = str(
        artifact_root.resolve()
    )
    request_path = artifact_root / "request.json"
    output_path = artifact_root / "output.json"
    write_private_json(request_path, request)
    run(
        [
            str(qualification_cli),
            "internal-embedding-qualification",
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=qualification_env,
        cwd=root,
        timeout=args.timeout_secs,
    )
    require(output_path.is_file() and not output_path.is_symlink(), "qualification runner omitted its output")
    output_bytes = output_path.read_bytes()
    for forbidden in [nonce, *projects]:
        require(
            forbidden.encode("utf-8") not in output_bytes,
            "qualification runner output leaked private request material",
        )
    try:
        output = json.loads(output_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"qualification runner output is not valid JSON: {exc}") from exc
    require(isinstance(output, dict), "qualification runner output must be an object")
    require_exact_keys(
        output,
        {
            "schema_version",
            "tier",
            "source",
            "package",
            "contracts",
            "runtime",
            "request_sha256",
            "scenarios",
            "measurements",
        },
        "qualification runner output",
    )
    require(output["schema_version"] == 2, "qualification runner schema is unsupported")
    require(output["tier"] == args.proof_tier, "qualification runner returned the wrong proof tier")
    expected_status = "calibration" if args.proof_tier == "calibration" else "pass"
    require(output["source"] == manifest["source"], "qualification runner source identity is stale")
    require(output["package"] == package, "qualification runner package identity is stale")
    require(output["contracts"] == contracts, "qualification runner contract identity is stale")
    require(output["runtime"] == request["runtime"], "qualification runner runtime identity is stale")
    expected_request_sha256 = hashlib.sha256(request_path.read_bytes()).hexdigest()
    require(
        output["request_sha256"] == expected_request_sha256,
        "qualification runner output is not bound to the exact private request",
    )

    constants_frozen = measurement_contract["constant_set"]["status"] == "frozen"
    shared = runtime["shared_identity"]
    require_exact_keys(
        shared,
        {
            "endpoint_namespace_id",
            "lifetime_authority_id",
            "listener_id",
            "server_instance_id",
            "server_process_start_id",
            "engine_owner_id",
            "native_worker_id",
            "load_generation",
            "model_load_count",
        },
        "live two-host shared identity",
    )
    for field in (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "server_instance_id",
        "server_process_start_id",
        "engine_owner_id",
        "native_worker_id",
    ):
        require_nonempty_string(shared[field], f"live shared_identity.{field}")
    require_positive_int(shared["load_generation"], "live shared_identity.load_generation")
    require(
        shared["model_load_count"] == 1,
        "qualification runner cold race did not preserve one model load",
    )

    scenario_contracts = measurement_contract["measurement_protocol"]["scenario_contracts"]
    raw_scenarios = output["scenarios"]
    require(
        isinstance(raw_scenarios, dict) and set(raw_scenarios) == REQUIRED_SERVER_SCENARIOS,
        "qualification runner returned an incomplete scenario set",
    )
    retained_scenarios: dict[str, dict] = {}
    forbidden_values = [nonce, *projects]
    for scenario_id in sorted(REQUIRED_SERVER_SCENARIOS):
        required_assertions = set(scenario_contracts[scenario_id]["required"])
        artifact, assertions = qualification_artifact(
            artifact_root,
            raw_scenarios[scenario_id],
            scenario_id=scenario_id,
            contracts=contracts,
            package=package,
            same_account=runtime["same_account"],
            materialization=runtime["materialization"],
            nonce_sha256=nonce_sha256,
            forbidden_values=forbidden_values,
        )
        external_artifacts = []
        if scenario_id in {"server_crash", "worker_stall"}:
            for assertion, value in publication_fault_evidence["assertions"].items():
                require(
                    assertion in required_assertions,
                    f"publication fault evidence derived unknown assertion {assertion}",
                )
                assertions[assertion] = value
            external_artifacts.append(publication_fault_evidence["artifact"])
            if (
                scenario_id == "server_crash"
                and fault_recovery_consistency_evidence is not None
            ):
                external_artifacts.append(
                    fault_recovery_consistency_evidence["artifact"]
                )
        require(
            set(assertions) == required_assertions,
            f"qualification scenario {scenario_id} derived assertion set differs from its preregistered contract",
        )
        require(
            all(value is True for value in assertions.values()),
            f"qualification scenario {scenario_id} has a failed assertion",
        )
        retained_scenarios[scenario_id] = {
            "status": "pass",
            "assertions": assertions,
            "artifacts": [artifact, *external_artifacts],
        }

    measurement = qualification_measurement_artifact(
        artifact_root,
        output["measurements"],
        contracts=contracts,
        measurement_contract=measurement_contract,
        target=manifest["asset_target"],
        proof_tier=args.proof_tier,
        matrix_cell_id=matrix_cell_id,
        expected_policy=args.engine_policy,
        expected_backend=expected_backend,
        forbidden_values=forbidden_values,
    )
    memory_evidence = retain_five_process_memory_evidence(
        artifact_root,
        runtime.get("_memory_observations"),
        source=manifest["source"],
        package=package,
        contracts=contracts,
        protocol=measurement_contract["measurement_protocol"],
        target=manifest["asset_target"],
        proof_tier=args.proof_tier,
        matrix_cell_id=matrix_cell_id,
        expected_policy=args.engine_policy,
        expected_backend=expected_backend,
        forbidden_values=forbidden_values,
    )
    timing = {
        "clock_domain": "awake_monotonic",
        "cross_process_timestamp_subtraction": False,
        "unplanned_suspend": measurement["unplanned_suspend"],
        "constants_frozen_before_run": constants_frozen,
        "constant_set_sha256": contracts["constant_set_sha256"],
    }
    cache_state = (
        "reused"
        if runtime["materialization"]["reused_on_rejoin"] is True
        else "materialized"
    )
    residency_state = require_nonempty_string(
        identity["embedding_engine_residency"],
        "runtime engine residency",
    )
    platform_label = (
        f"{sys.platform}:{os.uname().machine}"
        if hasattr(os, "uname")
        else sys.platform
    )
    host = {
        "fingerprint": canonical_sha256(
            {
                "platform": platform_label,
                "target": manifest["asset_target"],
                "account_id": runtime["same_account"]["account_id"],
                "backend": normalized_backend(expected_backend),
                "policy": args.engine_policy,
            }
        ),
        "platform": platform_label,
        "target": manifest["asset_target"],
        "matrix_cell_id": matrix_cell_id,
        "host_class": matrix_cell["host_class"],
        "accelerator_claim": matrix_cell["accelerator_claim"],
        "backend": identity["embedding_backend"],
        "policy": args.engine_policy,
        "cache_state": cache_state,
        "residency_state": residency_state,
        "unplanned_suspend": measurement["unplanned_suspend"],
    }
    nonclaims = {
        claim: {
            "claimed": False,
            "reason": (
                "this exact-package qualification tier does not establish the broader claim"
            ),
        }
        for claim in sorted(LOWER_TIER_NONCLAIMS)
    }

    required_metrics = set(measurement_contract["measurement_protocol"]["required_metrics"])
    metric_contracts = measurement_contract["measurement_protocol"]["metric_contracts"]
    thresholds = measurement_contract["constant_set"]["qualification_thresholds"]
    retained_metrics: dict[str, dict] = {}
    for metric in sorted(required_metrics):
        unit = metric_contracts[metric]["unit"]
        if metric == "retrieval_quality" and retrieval_quality_evidence is None:
            require(
                args.proof_tier == "calibration",
                "qualification retrieval quality omitted publishable packet evidence",
            )
            retained_metrics[metric] = {
                "status": "not_measured",
                "unit": unit,
                "value": None,
                "reason": (
                    "calibration omitted the separately produced exact-head publishable packet artifact"
                ),
            }
            continue
        value = (
            retrieval_quality_evidence["publishable_packet_pass_rate"]
            if metric == "retrieval_quality"
            else (
                memory_evidence["value"]
                if metric == "total_codestory_process_memory"
                else measurement["values"][metric]
            )
        )
        require(
            isinstance(value, (int, float)) and not isinstance(value, bool),
            f"qualification metric {metric} is not numeric",
        )
        if args.proof_tier == "calibration":
            retained_metrics[metric] = {
                "status": "calibration",
                "unit": unit,
                "value": value,
            }
            if metric == "retrieval_quality":
                retained_metrics[metric]["raw_evidence"] = retrieval_quality_evidence
            elif metric == "total_codestory_process_memory":
                retained_metrics[metric]["raw_evidence"] = memory_evidence["artifact"]
            else:
                retained_metrics[metric]["raw_evidence"] = measurement["artifact"]
            continue
        threshold = thresholds[metric]
        require(
            isinstance(threshold, (int, float)) and not isinstance(threshold, bool),
            f"qualification metric {metric} has no frozen threshold",
        )
        comparison = metric_contracts[metric]["comparison"]
        require(
            metric_passes(value, threshold, comparison),
            f"qualification metric {metric} failed its frozen threshold",
        )
        retained_metrics[metric] = {
            "status": "pass",
            "unit": unit,
            "value": value,
            "threshold": threshold,
            "comparison": comparison,
        }
        if metric == "retrieval_quality":
            require(
                retrieval_quality_evidence is not None,
                "qualification retrieval quality omitted publishable packet evidence",
            )
            retained_metrics[metric]["raw_evidence"] = retrieval_quality_evidence
        elif metric == "total_codestory_process_memory":
            retained_metrics[metric]["raw_evidence"] = memory_evidence["artifact"]
        else:
            retained_metrics[metric]["raw_evidence"] = measurement["artifact"]

    retained = {
        "schema_version": 1,
        "status": expected_status,
        "tier": args.proof_tier,
        "source": manifest["source"],
        "package": {
            **package,
            **contracts,
            "matrix_cell_id": matrix_cell_id,
            "accelerator_claim": matrix_cell["accelerator_claim"],
            "model_sha256": identity["embedding_model_sha256"],
            "backend": identity["embedding_backend"],
            "policy": identity["embedding_policy"],
            "cache_state": host["cache_state"],
            "residency_state": host["residency_state"],
        },
        "host": host,
        "same_account": runtime["same_account"],
        "shared_identity": shared,
        "timing": timing,
        "scenarios": retained_scenarios,
        "lower_tier_nonclaims": nonclaims,
        "metrics": retained_metrics,
    }
    if args.proof_tier == "installed_runtime":
        retained["installed_plugin"] = runtime["installed_plugin"]
        retained["managed_runtime"] = runtime["managed_runtime"]
    write_json(args.qualification_evidence, retained)
    assert_retained_json_privacy(
        args.qualification_evidence,
        [*forbidden_values, *runtime.get("_qualification_forbidden_values", [])],
    )
    if args.proof_tier == "calibration" and args.calibration_run_output is not None:
        run_index = require_positive_int(
            args.calibration_run_index,
            "--calibration-run-index",
        )
        require(
            run_index <= 3,
            "--calibration-run-index must be in the preregistered range 1..3",
        )
        normalized_package = {
            "archive_sha256": archive_sha256,
            "executable_sha256": manifest["binary"]["sha256"],
            "asset_target": manifest["asset_target"],
            "release_version": manifest["release_version"],
            "model_sha256": identity["embedding_model_sha256"],
            "policy": args.engine_policy,
            "backend": expected_backend,
        }
        run_contracts = {
            "protocol_sha256": measurement_contract["protocol_sha256"],
            "measurement_protocol_sha256": measurement_contract[
                "measurement_protocol_sha256"
            ],
            "input_constant_set_sha256": measurement_contract[
                "constant_set_sha256"
            ],
        }
        raw_metrics = json.loads(
            json.dumps(measurement["payload"]["metrics"])
        )
        memory_samples = []
        for sample in memory_evidence["payload"]["samples"]:
            normalized_sample = json.loads(json.dumps(sample))
            normalized_sample["process"] = normalized_sample.pop("producer_process")
            memory_samples.append(normalized_sample)
        raw_metrics["total_codestory_process_memory"] = {
            "unit": "bytes",
            "samples": memory_samples,
        }
        run_identity_seed = {
            "source": manifest["source"],
            "package": normalized_package,
            "matrix_cell_id": matrix_cell_id,
            "run_index": run_index,
            "host_fingerprint": host["fingerprint"],
            "measurement_artifact_sha256": measurement["artifact"]["sha256"],
            "memory_artifact_sha256": memory_evidence["artifact"]["sha256"],
        }
        run_id = canonical_sha256(run_identity_seed)
        raw_payload = {
            "schema_version": 1,
            "run_id_sha256": run_id,
            "matrix_cell_id": matrix_cell_id,
            "run_index": run_index,
            "host_fingerprint": host["fingerprint"],
            "source": manifest["source"],
            "contracts": run_contracts,
            "package": normalized_package,
            "clean": manifest["source"]["tracked_dirty"] is False,
            "unplanned_suspend": measurement["unplanned_suspend"],
            "metrics": raw_metrics,
        }
        run_envelope = {
            "run_id_sha256": run_id,
            "matrix_cell_id": matrix_cell_id,
            "run_index": run_index,
            "host_fingerprint": host["fingerprint"],
            "clean": raw_payload["clean"],
            "unplanned_suspend": raw_payload["unplanned_suspend"],
            "source": manifest["source"],
            "contracts": run_contracts,
            "package": normalized_package,
            "raw_artifact": {
                "name": "measurements.raw.json",
                "sha256": canonical_sha256(raw_payload),
                "payload": raw_payload,
            },
        }
        write_json(args.calibration_run_output, run_envelope)
    return retained


def build_calibration_self_test_bundle(
    root: Path,
    measurement_contract: dict,
    *,
    source: dict,
) -> tuple[Path, dict, dict]:
    protocol = measurement_contract["measurement_protocol"]
    contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "measurement_protocol_sha256": measurement_contract[
            "measurement_protocol_sha256"
        ],
        "input_constant_set_sha256": measurement_contract["constant_set_sha256"],
    }
    runs = []
    run_artifact_digests = []
    roles = (
        "plugin_host_a",
        "plugin_cli_a",
        "plugin_host_b",
        "plugin_cli_b",
        "embedding_server",
    )
    for cell_position, (matrix_cell_id, cell) in enumerate(
        sorted(protocol["calibration_matrix"].items())
    ):
        target_os = TARGET_CONTRACTS[cell["asset_target"]]["target_os"]
        awake_api = protocol["clock_policy"]["platform_apis"][target_os][0]
        inclusive_api = protocol["clock_policy"]["suspend_detection"]["platform_apis"][
            target_os
        ]
        for run_index in range(1, 4):
            run_seed = f"{matrix_cell_id}:{run_index}"
            run_id = hashlib.sha256(f"run:{run_seed}".encode()).hexdigest()
            host_fingerprint = hashlib.sha256(
                f"host:{matrix_cell_id}".encode()
            ).hexdigest()
            metrics = {}
            for metric_position, metric in enumerate(
                sorted(set(protocol["required_metrics"]) - {"retrieval_quality"})
            ):
                samples = []
                policy = protocol["metric_sampling"][metric]
                shared_server_id = "server:" + hashlib.sha256(
                    f"{run_seed}:{metric}".encode()
                ).hexdigest()
                for repeat in range(1, policy["sample_count"] + 1):
                    pid = (
                        10_000
                        + cell_position * 1_000
                        + run_index * 100
                        + metric_position * 10
                        + repeat
                    )
                    independent = (
                        policy.get("independence")
                        == "distinct_server_instance_per_sample"
                    )
                    server_id = (
                        "server:"
                        + hashlib.sha256(
                            f"{run_seed}:{metric}:{repeat}".encode()
                        ).hexdigest()
                        if independent
                        else shared_server_id
                    )
                    server_process_start_id = (
                        f"boot:{pid}"
                        if independent
                        else "boot:"
                        + hashlib.sha256(
                            f"server-start:{run_seed}:{metric}".encode()
                        ).hexdigest()
                    )
                    started_ns = repeat * 2_000_000
                    finished_ns = started_ns + 1_000_000
                    operands: dict[str, object] = {}
                    if metric in {
                        "cold_first_vector",
                        "first_product_ready",
                        "warm_query_ipc",
                        "warm_bulk_ipc",
                        "bulk_documents_per_second",
                        "bulk_tokens_per_second",
                    }:
                        operands["successful_operation_duration_ns"] = (
                            finished_ns - started_ns
                        )
                    if metric == "bulk_documents_per_second":
                        operands["completed_documents"] = 1
                    elif metric == "bulk_tokens_per_second":
                        operands["completed_tokens"] = 1
                    elif metric == "total_codestory_process_memory":
                        operands["processes"] = [
                            {
                                "role": role,
                                "pid": pid + role_index + 1,
                                "process_start_id": f"boot:{pid + role_index + 1}",
                                "executable_sha256": hashlib.sha256(
                                    f"exe:{role}".encode()
                                ).hexdigest(),
                                "resident_bytes": 1,
                                "measurement_api": "self_test",
                            }
                            for role_index, role in enumerate(roles)
                        ]
                    elif metric == "backend_observed_accelerator_residency":
                        accelerated = cell["policy"] == "accelerated"
                        operands = {
                            "policy": cell["policy"],
                            "backend": cell["backend"],
                            "accelerator_execution_verified": accelerated,
                            "resident_accelerator_tensor_count": 1 if accelerated else 0,
                            "resident_accelerator_tensor_bytes": 1 if accelerated else 0,
                            "offloaded_layer_count": 1 if accelerated else 0,
                            "model_layer_count": 1,
                        }
                    samples.append(
                        {
                            "sample_id": "sample:"
                            + hashlib.sha256(
                                f"{run_seed}:{metric}:{repeat}".encode()
                            ).hexdigest(),
                            "repeat": repeat,
                            "matrix_cell_id": matrix_cell_id,
                            "workload_id": protocol["workloads"][metric][
                                "workload_id"
                            ],
                            "cache_state": cell["cache_state"],
                            "residency_state": cell["residency_state"],
                            "process": {
                                "pid": pid,
                                "process_start_id": f"boot:{pid}",
                            },
                            "server_identity": {
                                "server_instance_id": server_id,
                                "process_start_id": server_process_start_id,
                                "load_generation": 1,
                            },
                            "clock": {
                                "domain": "awake_monotonic",
                                "api": awake_api,
                                "boot_id": f"boot-{cell_position}",
                                "resolution_ns": 1,
                            },
                            "start": {
                                "phase": protocol["phase_boundaries"][metric][0],
                                "observed_ns": started_ns,
                            },
                            "end": {
                                "phase": protocol["phase_boundaries"][metric][1],
                                "observed_ns": finished_ns,
                            },
                            "operands": operands,
                            "suspend_witness": {
                                "awake_started_ns": started_ns,
                                "awake_finished_ns": finished_ns,
                                "inclusive_clock_api": inclusive_api,
                                "inclusive_started_ns": started_ns,
                                "inclusive_finished_ns": finished_ns,
                                "boot_id_started": f"boot-{cell_position}",
                                "boot_id_finished": f"boot-{cell_position}",
                            },
                        }
                    )
                metrics[metric] = {
                    "unit": protocol["metric_contracts"][metric]["unit"],
                    "samples": samples,
                }
            raw_payload = {
                "schema_version": 1,
                "run_id_sha256": run_id,
                "matrix_cell_id": matrix_cell_id,
                "run_index": run_index,
                "host_fingerprint": host_fingerprint,
                "source": source,
                "contracts": contracts,
                "package": {
                    "archive_sha256": hashlib.sha256(
                        f"archive:{matrix_cell_id}".encode()
                    ).hexdigest(),
                    "executable_sha256": hashlib.sha256(
                        f"executable:{matrix_cell_id}".encode()
                    ).hexdigest(),
                    "asset_target": cell["asset_target"],
                    "release_version": "0.0.0",
                    "model_sha256": hashlib.sha256(b"model").hexdigest(),
                    "policy": cell["policy"],
                    "backend": cell["backend"],
                },
                "clean": True,
                "unplanned_suspend": False,
                "metrics": metrics,
            }
            raw_digest = canonical_sha256(raw_payload)
            run_artifact_digests.append(raw_digest)
            runs.append(
                {
                    "run_id_sha256": run_id,
                    "matrix_cell_id": matrix_cell_id,
                    "run_index": run_index,
                    "host_fingerprint": host_fingerprint,
                    "clean": True,
                    "unplanned_suspend": False,
                    "source": source,
                    "contracts": contracts,
                    "package": raw_payload["package"],
                    "raw_artifact": {
                        "name": "measurements.raw.json",
                        "sha256": raw_digest,
                        "payload": raw_payload,
                    },
                }
            )
    selected_constants = {
        "connect_timeout_ms": 2,
        "spawn_convergence_timeout_ms": 2,
        "request_deadlines_ms": {
            "query_request_deadline_ms": 2,
            "bulk_replay_success_budget_ms": 2,
            "bulk_request_deadline_ms": 9,
        },
        "capacity_retry_policy": {
            "retry_after_ms": 1,
            "retry_class": "after_capacity_change",
            "retry_condition_source": "named_condition_from_typed_capacity_response",
        },
        "election_backoff_policy": {
            "initial_backoff_ms": 1,
            "maximum_backoff_ms": 1,
            "jitter": (
                "sha256(process_start_id||attempt) modulo inclusive "
                "[initial_backoff_ms,maximum_backoff_ms]"
            ),
        },
        "hard_native_no_progress_ms": 4,
        "watchdog_cadence_ms": 1,
    }
    thresholds = {
        metric: (
            (1.0 if metric == "retrieval_quality" else 1)
            if metric in {"backend_observed_accelerator_residency", "retrieval_quality"}
            else (
                800
                if metric
                in {"bulk_documents_per_second", "bulk_tokens_per_second"}
                else (6 if metric == "total_codestory_process_memory" else 2)
            )
        )
        for metric in protocol["required_metrics"]
    }
    producer = {
        "repository": "TheGreenCedar/CodeStory",
        "workflow_path": ".github/workflows/packaged-platform-pr.yml",
        "run_id": "123",
        "run_attempt": "1",
        "artifact_name": f"embedding-calibration-bundle-{source['commit']}",
        "source_head_sha": source["commit"],
    }
    freeze_payload = {
        "selection_protocol": measurement_contract["constant_set"][
            "selection_protocol"
        ],
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "run_artifact_sha256s": sorted(run_artifact_digests),
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
    }
    bundle = {
        "schema_version": 1,
        "selection_protocol": measurement_contract["constant_set"][
            "selection_protocol"
        ],
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "runs": runs,
        "freeze_digest": canonical_sha256(freeze_payload),
    }
    path = root / "calibration-bundle.json"
    write_json(path, bundle)
    frozen_contract = json.loads(json.dumps(measurement_contract))
    frozen_contract["constant_set"]["status"] = "frozen"
    frozen_contract["constant_set"]["calibration_required_values"] = selected_constants
    frozen_contract["constant_set"]["qualification_thresholds"] = thresholds
    frozen_contract["constant_set"]["freeze_record"] = {
        "selection_source_commit": source["commit"],
        "selection_source_tree": source["tree"],
        "measurement_protocol_sha256": contracts["measurement_protocol_sha256"],
        "protocol_sha256": contracts["protocol_sha256"],
        "input_constant_set_sha256": contracts["input_constant_set_sha256"],
        "calibration_bundle_sha256": sha256(path),
        "calibration_freeze_digest": bundle["freeze_digest"],
        "run_artifact_sha256s": sorted(run_artifact_digests),
        "selection_rule": "all_preregistered_clean_runs_no_outlier_removal",
        "selected_at": "self-test",
    }
    return path, frozen_contract, bundle


def self_test() -> None:
    require(parse_byte_quantity("24.1M") == 25_270_682, "memory quantity parser failed")
    def fail_parallel(message: str) -> None:
        raise ProofFailure(message)

    try:
        run_parallel(
            {
                "z-task": lambda: fail_parallel("z failed"),
                "a-task": lambda: fail_parallel("a failed"),
            }
        )
    except ProofFailure as error:
        require(
            str(error)
            == "parallel qualification tasks failed: a-task: a failed; z-task: z failed",
            "parallel qualification failure aggregation is unstable",
        )
    else:
        raise ProofFailure("parallel qualification failures were ignored")

    publication_status = {
        "retrieval_mode": "full",
        "manifest_contract": {
            "project_id": "repo-v2-self-test",
            "input_hash": "1" * 64,
            "generation": "repo-v2-self-test-generation",
            "schema_version": 6,
            "graph_hash": "2" * 64,
        },
        "manifest": {
            "project_id": "repo-v2-self-test",
            "sidecar_input_hash": "1" * 64,
            "sidecar_generation": "repo-v2-self-test-generation",
            "sidecar_schema_version": 6,
            "graph_artifact_hash": "2" * 64,
            "lexical_version": "sqlite-fts5-v1",
            "semantic_generation": "semantic-self-test",
            "scip_revision": "graph-self-test",
        },
    }
    publication_identity = publication_identity_from_status(publication_status)
    require_sha256(publication_identity, "publication identity self-test")
    hostile_publication_status = json.loads(json.dumps(publication_status))
    hostile_publication_status["manifest"]["sidecar_generation"] = "stale-generation"
    try:
        publication_identity_from_status(hostile_publication_status)
    except ProofFailure:
        pass
    else:
        raise ProofFailure("manifest report/contract drift was accepted")

    require(
        require_native_process_start_identity(
            "linux:1234", "linux", "Linux self-test identity"
        )
        == "linux:1234"
        and require_native_process_start_identity(
            "macos-proc:1234:5678", "macos", "macOS self-test identity"
        )
        == "macos-proc:1234:5678"
        and require_native_process_start_identity(
            "windows:504911232000000010",
            "windows",
            "Windows self-test identity",
        )
        == "windows:504911232000000010",
        "canonical process identity format self-test failed",
    )
    for target_os, hostile_identity in (
        ("linux", "boot-id:1234"),
        ("macos", "Thu Jul 17 12:00:00 2026"),
        ("windows", "2026-07-17T12:00:00Z"),
    ):
        try:
            require_native_process_start_identity(
                hostile_identity,
                target_os,
                f"hostile {target_os} identity",
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"noncanonical {target_os} process identity format was accepted"
            )
    self_target_os = (
        "windows"
        if os.name == "nt"
        else ("macos" if sys.platform == "darwin" else "linux")
    )
    self_pid = os.getpid()
    self_start_id = process_start_identity(self_pid)
    self_live_digest = live_process_executable_sha256(
        self_pid,
        self_start_id,
        self_target_os,
    )
    verified_live_executable(
        pid=self_pid,
        process_start_id=self_start_id,
        reported_sha256=self_live_digest,
        expected_sha256=self_live_digest,
        target_os=self_target_os,
        label="self-test process",
    )
    hostile_reported_digest = (
        ("a" if self_live_digest[0] != "a" else "b") + self_live_digest[1:]
    )
    try:
        verified_live_executable(
            pid=self_pid,
            process_start_id=self_start_id,
            reported_sha256=hostile_reported_digest,
            expected_sha256=self_live_digest,
            target_os=self_target_os,
            label="hostile self-test process",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("self-reported process executable digest bypassed live image hashing")
    stale_start_id = self_start_id[:-1] + (
        "0" if self_start_id[-1] != "0" else "1"
    )
    try:
        live_process_executable_sha256(
            self_pid,
            stale_start_id,
            self_target_os,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("stale process start identity bypassed live image hashing")

    class ScriptedMcpProcess(McpProcess):
        def __init__(self, responses: list[dict]):
            self.timeout = 1
            self.responses = iter(responses)
            self.calls: list[tuple[str, dict, str]] = []
            self.tool_attempt_counts: dict[str, int] = {}

        def tool(self, name: str, arguments: dict, request_id: str) -> dict:
            self.calls.append((name, arguments, request_id))
            try:
                return next(self.responses)
            except StopIteration as exc:
                raise ProofFailure("scripted MCP response sequence was exhausted") from exc

    projection_fixture_path = (
        Path(__file__).resolve().parents[2]
        / "crates"
        / "codestory-cli"
        / "tests"
        / "fixtures"
        / "stdio_installed_host_search_retrieval.json"
    )
    projection_fixture = json.loads(projection_fixture_path.read_text(encoding="utf-8"))
    require(
        isinstance(projection_fixture, dict),
        f"installed search projection fixture is not an object: {projection_fixture!r}",
    )
    ready_retrieval = projection_fixture.get("projected")
    require(
        isinstance(ready_retrieval, dict),
        f"installed search projection fixture is missing projected retrieval: {projection_fixture!r}",
    )

    query = "scripted-search"
    preparing = {
        "result": {
            "isError": True,
            "structuredContent": {
                "code": "codestory_preparing",
                "state": "preparing",
                "retry_tool": "search",
                "retry_after_ms": 0,
            },
        }
    }
    ready = {
        "result": {
            "structuredContent": {
                "query": query,
                "hits": [],
                "retrieval": ready_retrieval,
            }
        }
    }
    scripted = ScriptedMcpProcess([preparing, ready])
    _, attempts = scripted.search_until_ready({"query": query}, "self-test-search")
    require(attempts == 2, "preparing search did not converge on its second attempt")
    require(
        scripted.tool_attempt_counts.get("self-test-search") == 2,
        "preparing search attempt count was not retained",
    )

    unavailable = ScriptedMcpProcess([
        {
            "result": {
                "isError": True,
                "structuredContent": {
                    "code": "codestory_unavailable",
                    "state": "unavailable",
                    "message": "hostile terminal response",
                },
            }
        }
    ])
    try:
        unavailable.search_until_ready({"query": query}, "self-test-unavailable")
    except ProofFailure as exc:
        require(
            "codestory_unavailable" in str(exc),
            f"terminal MCP failure omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("terminal MCP unavailable response was retried or accepted")
    require(len(unavailable.calls) == 1, "terminal MCP unavailable response was retried")

    hostile_search_results = [
        (
            "legacy mode=full",
            {"query": query, "hits": [], "retrieval": {"mode": "full"}},
            "ready installed retrieval projection",
        ),
        (
            "preparing retrieval projection",
            {"query": query, "hits": [], "retrieval": {"state": "preparing"}},
            "ready installed retrieval projection",
        ),
        (
            "missing retrieval projection",
            {"query": query, "hits": []},
            "ready installed retrieval projection",
        ),
        (
            "non-array hits",
            {"query": query, "hits": {}, "retrieval": ready_retrieval},
            "non-array hits",
        ),
    ]
    for label, structured_content, expected_diagnostic in hostile_search_results:
        hostile = ScriptedMcpProcess(
            [{"result": {"structuredContent": structured_content}}]
        )
        try:
            hostile.search_until_ready({"query": query}, f"self-test-{label}")
        except ProofFailure as exc:
            require(
                expected_diagnostic in str(exc),
                f"{label} failure omitted its diagnostics: {exc}",
            )
        else:
            raise ProofFailure(f"{label} search result was accepted")
        require(len(hostile.calls) == 1, f"{label} search result was retried")

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        self_protocol = root / SERVER_PROTOCOL.name
        self_protocol.write_bytes(SERVER_PROTOCOL.read_bytes())
        self_constant_set = root / SERVER_CONSTANT_SET.name
        unfrozen_constant_set = json.loads(
            SERVER_CONSTANT_SET.read_text(encoding="utf-8")
        )
        unfrozen_constant_set["status"] = "unfrozen"
        unfrozen_constant_set["calibration_required_values"] = {
            field: None
            for field in unfrozen_constant_set["calibration_required_values"]
        }
        unfrozen_constant_set["qualification_thresholds"] = {
            field: None
            for field in unfrozen_constant_set["qualification_thresholds"]
        }
        unfrozen_constant_set["freeze_record"] = None
        write_json(self_constant_set, unfrozen_constant_set)
        self_measurement_protocol = root / MEASUREMENT_PROTOCOL.name
        self_measurement_protocol.write_bytes(MEASUREMENT_PROTOCOL.read_bytes())
        retained_privacy_path = root / "retained-privacy.json"
        write_json(
            retained_privacy_path,
            {"account_id": "account:" + "a" * 64, "transcript_sha256": "b" * 64},
        )
        assert_retained_json_privacy(retained_privacy_path, [str(root), "private query"])
        write_json(retained_privacy_path, {"request_text": "private query"})
        try:
            assert_retained_json_privacy(retained_privacy_path, [str(root), "private query"])
        except ProofFailure:
            pass
        else:
            raise ProofFailure("retained evidence private request text was accepted")
        payload = root / "artifact.zip"
        binary_header = bytearray(64)
        binary_header[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", binary_header, 4, 0x0100000C)
        model = {
            "file_name": "test.gguf",
            "size_bytes": 4,
            "sha256": "a" * 64,
            "embedded": True,
            "producer": {"name": "test", "version": "0.0.0"},
        }
        embedding = {
            "family": "inprocess:test",
            "dimension": 768,
            "query_prefix": "query: ",
            "document_prefix": "",
            "pooling": "cls",
            "normalization": "l2",
            "element_type": "f32_le",
            "vector_schema_version": 2,
        }
        tokenizer = {
            "container": "gguf",
            "tokenizer_sha256": "b" * 64,
            "config_sha256": "c" * 64,
        }
        contract_sha256 = embedding_contract_digest(model, embedding, tokenizer)
        build_identity = (
            "codestory-native-engine-v1|target=aarch64-apple-darwin|os=macos|"
            "arch=aarch64|linkage=static|backend_loading=builtin|"
            "backends=cpu,metal|llama_cpp_crate=test|"
            f"llama_cpp_commit=test|model_sha256={'a' * 64}|"
            f"embedding_contract_sha256={contract_sha256}|model_embedded=true|"
            "producer=test@0.0.0|end"
        )
        protocol_sha256 = sha256(self_protocol)
        constant_set_sha256 = sha256(self_constant_set)
        measurement_protocol_sha256 = sha256(self_measurement_protocol)
        server_proof_identity = (
            "codestory-embedding-server-proof-v1|bootstrap=1|protocol_schema=1|"
            f"protocol_sha256={protocol_sha256}|"
            f"constant_set_sha256={constant_set_sha256}|"
            f"measurement_protocol_sha256={measurement_protocol_sha256}|"
            "clock_policy=awake_monotonic|query_capacity=64|bulk_capacity=64|"
            "idle_timeout_ms=60000|end"
        )
        binary_payload = (
            bytes(binary_header)
            + b"\0"
            + build_identity.encode("ascii")
            + b"\0"
            + server_proof_identity.encode("ascii")
            + b"\0"
        )
        server_proof = parse_server_proof_identity(server_proof_identity)
        server_proof["constant_set_status"] = "unfrozen"
        valid_manifest = {
            "schema_version": 3,
            "release_version": "0.0.0",
            "asset_target": "macos-arm64",
            "source": {
                "commit": "1" * 40,
                "tree": "2" * 40,
                "tracked_dirty": False,
            },
            "binary": {
                "name": "codestory-cli",
                "sha256": hashlib.sha256(binary_payload).hexdigest(),
                "format": "mach-o",
                "arch": "aarch64",
                "needed": [],
            },
            "runtime_artifacts": [],
            "engine": {
                "build_contract_schema_version": 2,
                "build_identity": build_identity,
                "target_triple": "aarch64-apple-darwin",
                "target_os": "macos",
                "target_arch": "aarch64",
                "linkage": "static",
                "backend_loading": "builtin",
                "compiled_backends": ["cpu", "metal"],
                "llama_cpp_crate_version": "test",
                "llama_cpp_source_commit": "test",
                "embedding_contract_sha256": contract_sha256,
            },
            "model": model,
            "embedding": embedding,
            "tokenizer_config": tokenizer,
            "accelerator": {
                "cpu_fallback": "explicit_only",
                "package_claim": "compiled_capability_only",
                "runtime_execution": "not_proven_by_package",
                "expected_protected_backend": "metal",
                "non_claim_reason": None,
            },
            "server_proof": server_proof,
        }
        with zipfile.ZipFile(payload, "w") as handle:
            handle.writestr("codestory-cli", binary_payload)
            handle.writestr(
                NATIVE_MANIFEST_FILE,
                json.dumps(valid_manifest, indent=2, sort_keys=True) + "\n",
            )
        checksum = root / "SHA256SUMS.txt"
        checksum.write_text(f"{sha256(payload)}  {payload.name}\n", encoding="utf-8")
        require(expected_archive_digest(checksum, payload) == sha256(payload), "checksum parser failed")
        unpacked = root / "unpacked"
        unpack_archive(payload, unpacked)
        cli = find_cli(unpacked)
        require(cli.name == "codestory-cli", "CLI discovery failed")
        manifest = load_native_manifest(unpacked, cli, "0.0.0")
        require(manifest["engine"]["linkage"] == "static", "manifest validation failed")
        malicious = root / "malicious.zip"
        with zipfile.ZipFile(malicious, "w") as handle:
            handle.writestr("../outside", b"bad")
        try:
            unpack_archive(malicious, root / "bad")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("archive traversal was accepted")
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
            control_actions=[],
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
                control_actions=[],
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
                control_actions=[],
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
        true_idle_before = json.loads(json.dumps(first_snapshot))
        true_idle_respawned = json.loads(json.dumps(first_snapshot))
        true_idle_respawned["process"]["server_instance_id"] = "respawned-server"
        true_idle_respawned["process"]["pid"] = 303
        true_idle_respawned["process"]["process_start_id"] = "process-start-303"
        true_idle_respawned["authority"]["lifetime_authority_id"] = (
            "respawned-authority"
        )
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
                    }
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
                    }
                }
            ],
        }
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
            control_actions=[],
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
                    control_actions=[],
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
                raise ProofFailure(
                    f"hostile true-idle {field} escaped replacement binding"
                )
        historical_only_invocations = json.loads(json.dumps(true_idle_invocations[1:]))
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=true_idle_transitions,
                process_observations=true_idle_process_observations,
                invocations=historical_only_invocations,
                control_actions=[],
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
                control_actions=[],
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
                control_actions=[],
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
                control_actions=[],
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
        measurement_contract = verify_package_server_contracts(
            manifest,
            self_measurement_protocol,
            require_frozen=False,
        )
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
        hostile_metric["samples"][0]["operands"].pop(
            "successful_operation_duration_ns"
        )
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
        duplicate_run["raw_artifact"]["payload"]["metrics"]["warm_query_ipc"][
            "samples"
        ][0]["sample_id"] = first_sample_id
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
        self_digest = lambda label: hashlib.sha256(label.encode("utf-8")).hexdigest()
        external_package = {
            "archive_sha256": "b" * 64,
            "executable_sha256": manifest["binary"]["sha256"],
            "asset_target": manifest["asset_target"],
            "release_version": manifest["release_version"],
        }
        external_contracts = {
            "protocol_sha256": protocol_sha256,
            "constant_set_sha256": constant_set_sha256,
            "measurement_protocol_sha256": measurement_protocol_sha256,
        }
        correlation_id = "0123456789abcdef0123456789abcdef"
        previous_publication = self_digest("previous-publication")
        publication_payload = {
            "schema_version": 1,
            "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
            "source": manifest["source"],
            "package": external_package,
            "contracts": external_contracts,
            "correlation_id": correlation_id,
            "previous_publication_identity_sha256": previous_publication,
            "server_observations": [
                {
                    "phase": "before_crash",
                    "server_instance_id": "server-before",
                    "process_start_id": "boot-1:101",
                    "load_generation": 1,
                },
                {
                    "phase": "after_replacement",
                    "server_instance_id": "server-after",
                    "process_start_id": "boot-1:102",
                    "load_generation": 1,
                },
            ],
            "candidate_observation": {
                "command": "retrieval_index",
                "exit_code": 1,
                "stdout_sha256": self_digest("candidate-stdout"),
                "stderr_sha256": self_digest("candidate-stderr"),
            },
            "publication_hook_events": [
                {
                    "schema_version": 1,
                    "sequence": index,
                    "correlation_id": correlation_id,
                    "action": action,
                    "status": status,
                    "clock": {
                        "domain": "process_monotonic",
                        "api": "std::time::Instant",
                        "elapsed_ns": index,
                    },
                }
                for index, (action, status) in enumerate(
                    (
                        ("pause_before_manifest_commit", "waiting_for_resume"),
                        ("resume_manifest_commit", "observed"),
                        ("lease_revalidation", "failed"),
                        ("manifest_commit", "returned_error"),
                    )
                )
            ],
            "ordinary_product_observations": [
                {
                    "sequence": index,
                    "command": command,
                    "exit_code": 0,
                    "retrieval_mode": "full",
                    "publication_identity_sha256": previous_publication,
                    "output_sha256": self_digest(f"{command}-output"),
                }
                for index, command in enumerate(("retrieval_status", "search"))
            ],
        }
        publication_path = root / "publication-fault.raw.json"
        write_json(publication_path, publication_payload)
        publication_external = verify_publication_fault_raw_evidence(
            publication_path,
            source=manifest["source"],
            package=external_package,
            contracts=external_contracts,
        )
        require(
            all(publication_external["assertions"].values()),
            "publication fault self-test did not derive its assertions",
        )
        hostile_publication = json.loads(json.dumps(publication_payload))
        hostile_publication["assertions"] = {"lost_publication_lease_blocks_commit": True}
        write_json(publication_path, hostile_publication)
        try:
            verify_publication_fault_raw_evidence(
                publication_path,
                source=manifest["source"],
                package=external_package,
                contracts=external_contracts,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("self-declared publication assertions were accepted")
        write_json(publication_path, publication_payload)
        scenario_observations = {
            "inflight_request_observed": [
                {
                    "values": {
                        "query_capacity": 64,
                        "query_depth": 0,
                        "bulk_capacity": 64,
                        "bulk_depth": 0,
                        "active_request_count": 1,
                        "lease_count": 0,
                        "active_request_class": "query",
                    }
                }
            ],
            "server_replaced": [
                {
                    "values": {
                        "old_server_instance_id": "server-before",
                        "new_server_instance_id": "server-after",
                    }
                }
            ],
            "query_replayed": [
                {
                    "values": {
                        "logical_operation_count": 1,
                        "wire_attempt_count": 2,
                        "wire_attempts": [
                            {
                                "ordinal": 1,
                                "request_id": "request-before",
                                "server_instance_id": "server-before",
                                "submitted_ns": 1,
                                "completed_ns": 2,
                                "outcome": "server_loss",
                            },
                            {
                                "ordinal": 2,
                                "request_id": "request-after",
                                "server_instance_id": "server-after",
                                "submitted_ns": 3,
                                "completed_ns": 4,
                                "outcome": "completed",
                            },
                        ],
                    }
                }
            ],
        }
        derived_server_crash = derive_scenario_assertions(
            "server_crash",
            observations_by_kind=scenario_observations,
            process_observations=[],
            invocations=[],
            control_actions=["hold_class", "crash_server"],
            same_account={},
            materialization={},
        )
        require(
            derived_server_crash
            == {
                "one_replacement_server": True,
                "pure_embedding_rpc_replayed_at_most_once": True,
            },
            "scenario assertion self-test did not derive exact raw claims",
        )
        hostile_scenario = json.loads(json.dumps(scenario_observations))
        hostile_scenario["query_replayed"][0]["values"]["wire_attempts"][1][
            "outcome"
        ] = "server_loss"
        try:
            derive_scenario_assertions(
                "server_crash",
                observations_by_kind=hostile_scenario,
                process_observations=[],
                invocations=[],
                control_actions=["hold_class", "crash_server"],
                same_account={},
                materialization={},
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("named scenario transitions with false values were accepted")

        consistency_payload = {
            "schema_version": 1,
            "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
            "source": manifest["source"],
            "package": external_package,
            "contracts": external_contracts,
            "run_id_sha256": self_digest("consistency-run"),
            "observations": [
                {
                    "case_id_sha256": self_digest(f"consistency-case-{index}"),
                    "before_server_fault_rank": 1,
                    "after_server_replacement_rank": 1,
                }
                for index in range(FAULT_RECOVERY_CONSISTENCY_CASES)
            ],
        }
        consistency_path = root / "fault-recovery-consistency.raw.json"
        write_json(consistency_path, consistency_payload)
        consistency_external = verify_fault_recovery_consistency_raw_evidence(
            consistency_path,
            source=manifest["source"],
            package=external_package,
            contracts=external_contracts,
        )
        require(
            consistency_external["ranks_stable"] is True,
            "fault recovery consistency self-test did not derive stable ranks",
        )
        hostile_consistency = json.loads(json.dumps(consistency_payload))
        hostile_consistency["observations"][0]["after_server_replacement_rank"] = 2
        write_json(consistency_path, hostile_consistency)
        try:
            verify_fault_recovery_consistency_raw_evidence(
                consistency_path,
                source=manifest["source"],
                package=external_package,
                contracts=external_contracts,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("changed fault-recovery search ranks were accepted")

        packet_row = {
            "repo": "fixture",
            "task_id": "quality-contract",
            "mode": "cold_cli_packet",
            "status": "pass",
            "quality": {"pass": True},
            "sufficiency": {
                "status": "sufficient",
                "sufficient_quality_mismatch": False,
                "follow_up_commands_count": 0,
                "open_next_count": 0,
                "gaps_count": 0,
                "coverage_unresolved_blocking_count": 0,
            },
            "packet_latency": {
                "sla_missed": False,
                "retrieval_shadow": {"retrieval_mode": "full"},
            },
            "repo_provenance": {
                "manifest_overridden_by_builtin": False,
                "configured": {
                    "url": "https://github.com/example/fixture.git",
                    "ref": "9" * 40,
                },
                "manifest": {
                    "url": "https://github.com/example/fixture.git",
                    "ref": "9" * 40,
                },
                "git_head": "9" * 40,
                "git_origin": "https://github.com/example/fixture.git",
                "git_dirty": False,
            },
            "codestory_cache_provenance": {
                "doctor_status": "pass",
                "storage_path": "fixture/codestory.db",
                "cache_policy": "prepared-retrieval-cache-read-only",
                "retrieval_mode": "full",
                "semantic_generation": "fixture-generation",
                "manifest_embedding_backend": "per-user-server:coderank-embed:q8_0",
                "semantic_backend": "coderank-embed",
                "embedding_engine_instance_id": "engine-fixture",
                "embedding_policy": "accelerated",
                "local_only": True,
                "indexed": True,
                "freshness_status": "fresh",
                "semantic_ready": True,
                "indexing_in_timed_run": False,
            },
        }
        holdout_tasks, _holdout_manifest_set_sha256 = load_holdout_task_contracts()
        quality_rows = []
        for (repo_name, task_id), task_contract in sorted(holdout_tasks.items()):
            for repeat in range(1, MIN_RETRIEVAL_QUALITY_REPEATS + 1):
                row = json.loads(json.dumps(packet_row))
                row["repo"] = repo_name
                row["task_id"] = task_id
                row["repeat"] = repeat
                row["task_manifest_snapshot"] = {
                    **task_contract["snapshot"],
                    "manifest_path": str(task_contract["path"]),
                }
                row["repo_provenance"]["configured"] = {
                    "url": task_contract["repo"]["url"],
                    "ref": task_contract["repo"]["ref"],
                    "languages": task_contract["repo"].get("languages", []),
                }
                row["repo_provenance"]["manifest"] = {
                    "url": task_contract["repo"]["url"],
                    "ref": task_contract["repo"]["ref"],
                    "workspace_root": task_contract["repo"].get("workspace_root"),
                }
                row["repo_provenance"]["git_head"] = task_contract["repo"]["ref"]
                row["repo_provenance"]["git_origin"] = task_contract["repo"]["url"]
                quality_rows.append(row)
        quality_payload = {
            "modes": ["cold-cli"],
            "repeats": MIN_RETRIEVAL_QUALITY_REPEATS,
            "release_evidence": {
                "commit": manifest["source"]["commit"],
                "source_tree": manifest["source"]["tree"],
                "evaluation_contract": RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
                "profile": "self-test",
                "evidence_identity": {
                    "corpus_id": RELEASE_QUALITY_CORPUS_ID,
                    "cache_id": "self-test-cache",
                    "machine_fingerprint": "self-test-host",
                },
                "publishable": True,
                "repeats": MIN_RETRIEVAL_QUALITY_REPEATS,
                "quality_gate_status": "pass",
                "publishable_blockers": [],
                "rows": quality_rows,
            },
        }
        quality_path = root / "packet-runtime-summary.json"
        write_json(quality_path, quality_payload)
        quality_external = verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
        require(
            quality_external["publishable_packet_pass_rate"] == 1,
            "retrieval quality self-test did not derive the packet pass rate",
        )
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["assertions"] = {"retrieval_quality": True}
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("self-declared retrieval quality pass was accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["rows"].pop()
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("incomplete retrieval quality repeats were accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["rows"] = [
            row
            for row in hostile_quality["release_evidence"]["rows"]
            if row["task_id"] == "axios-request-dispatch"
        ]
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("one-task retrieval quality subset was accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["rows"][0]["task_manifest_snapshot"][
            "prompt"
        ] = "hostile substituted task"
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("substituted retrieval quality task manifest was accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["source_tree"] = "f" * 40
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale retrieval quality source tree was accepted")
        write_json(quality_path, quality_payload)
        try:
            verify_package_server_contracts(
                manifest,
                self_measurement_protocol,
                require_frozen=True,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("unfrozen server constants were accepted for qualification")
        scenario_contracts = measurement_contract["measurement_protocol"]["scenario_contracts"]
        qualification_contract = json.loads(json.dumps(measurement_contract))
        qualification_contract["constant_set"]["qualification_thresholds"] = {
            metric: 1
            for metric in measurement_contract["measurement_protocol"]["required_metrics"]
        }
        retained = {
            "schema_version": 1,
            "status": "pass",
            "tier": "installed_runtime",
            "source": manifest["source"],
            "package": {
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
                "protocol_sha256": protocol_sha256,
                "constant_set_sha256": constant_set_sha256,
                "measurement_protocol_sha256": measurement_protocol_sha256,
            },
            "host": {
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
            },
            "installed_plugin": {
                "schema_version": 1,
                "installation_source": "marketplace",
                "marketplace_repository": "TheGreenCedar/AgentPluginMarketplace",
                "marketplace_commit": "d" * 40,
                "plugin_id": "codestory",
                "plugin_version": "0.0.0",
                "plugin_source_commit": manifest["source"]["commit"],
                "plugin_package_sha256": "e" * 64,
            },
            "managed_runtime": {
                "cli_source": "managed",
                "plugin_version": "0.0.0",
                "managed_binary_sha256": manifest["binary"]["sha256"],
                "archive_sha256": "b" * 64,
                "build_source": "github_release",
                "repo_ref": "v0.0.0",
                "provisioned_at": "self-test",
            },
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
            "shared_identity": shared,
            "timing": {
                "clock_domain": "awake_monotonic",
                "cross_process_timestamp_subtraction": False,
                "unplanned_suspend": False,
                "constants_frozen_before_run": True,
                "constant_set_sha256": constant_set_sha256,
            },
            "scenarios": {
                scenario_id: {
                    "status": "pass",
                    "assertions": {
                        assertion: True
                        for assertion in contract["required"]
                    },
                    "artifacts": [
                        {
                            "name": f"{scenario_id}.json",
                            "sha256": "c" * 64,
                        }
                    ],
                }
                for scenario_id, contract in scenario_contracts.items()
            },
            "lower_tier_nonclaims": {
                claim: {
                    "claimed": False,
                    "reason": "self-test lower-tier boundary",
                }
                for claim in LOWER_TIER_NONCLAIMS
            },
            "metrics": {
                metric: {
                    "status": "pass",
                    "unit": measurement_contract["measurement_protocol"]["metric_contracts"][metric]["unit"],
                    "value": 1,
                    "threshold": 1,
                    "comparison": measurement_contract["measurement_protocol"]["metric_contracts"][metric]["comparison"],
                }
                for metric in measurement_contract["measurement_protocol"]["required_metrics"]
            },
        }
        retained["scenarios"]["server_crash"]["artifacts"].append(
            publication_external["artifact"]
        )
        retained["scenarios"]["worker_stall"]["artifacts"].append(
            publication_external["artifact"]
        )
        retained["scenarios"]["server_crash"]["artifacts"].append(
            consistency_external["artifact"]
        )
        retained["metrics"]["retrieval_quality"]["raw_evidence"] = quality_external
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
        verify_retained_qualification(
            retained,
            manifest=manifest,
            archive_sha256="b" * 64,
            shared_identity=shared,
            measurement_contract=qualification_contract,
            required_tier="installed_runtime",
            required_matrix_cell_id="installed_macos_arm64_cpu",
            expected_policy="cpu_explicit",
            expected_backend="cpu",
            expected_accelerator_claim="none",
            installed_plugin=retained["installed_plugin"],
            managed_runtime=retained["managed_runtime"],
        )
        missing_scenario = json.loads(json.dumps(retained))
        missing_scenario["scenarios"].pop("frozen_owner")
        try:
            verify_retained_qualification(
                missing_scenario,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("incomplete installed scenario evidence was accepted")
        wrong_tier = json.loads(json.dumps(retained))
        wrong_tier["tier"] = "protected_hardware"
        try:
            verify_retained_qualification(
                wrong_tier,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("different-tier retained qualification was accepted")
        stale_shared = json.loads(json.dumps(retained))
        stale_shared["shared_identity"]["server_instance_id"] = "stale-server"
        try:
            verify_retained_qualification(
                stale_shared,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale retained shared server identity was accepted")
        wrong_cell = json.loads(json.dumps(retained))
        wrong_cell["package"]["matrix_cell_id"] = "protected_macos_arm64_metal"
        try:
            verify_retained_qualification(
                wrong_cell,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("wrong qualification matrix cell was accepted")
        invalid = {**valid, "embedding_adapter": "llvmpipe"}
        try:
            engine_identity(invalid, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("software adapter was accepted")
        inferred = {**valid, "embedding_execution_observation_source": "inferred_from_request"}
        try:
            engine_identity(inferred, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("inferred accelerator execution was accepted")

        hostile_root = root / "hostile-manifest"
        hostile_root.mkdir()
        hostile_cli = hostile_root / "codestory-cli"
        hostile_cli.write_bytes(binary_payload)
        hostile_manifest = json.loads(json.dumps(valid_manifest))
        hostile_manifest["binary"]["sha256"] = "0" * 64
        write_json(hostile_root / NATIVE_MANIFEST_FILE, hostile_manifest)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("binary/manifest digest mismatch was accepted")

        wrong_target = json.loads(json.dumps(valid_manifest))
        wrong_target["asset_target"] = "macos-x64"
        write_json(hostile_root / NATIVE_MANIFEST_FILE, wrong_target)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("asset target/binary architecture mismatch was accepted")

        stale_contract = json.loads(json.dumps(valid_manifest))
        stale_contract["embedding"]["query_prefix"] = "changed query: "
        write_json(hostile_root / NATIVE_MANIFEST_FILE, stale_contract)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale binary embedding contract was accepted")

        marker_mismatch = json.loads(json.dumps(valid_manifest))
        marker_mismatch["engine"]["build_identity"] = build_identity.replace(
            "|end", "|note=fabricated|end"
        )
        write_json(hostile_root / NATIVE_MANIFEST_FILE, marker_mismatch)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("binary/manifest native marker mismatch was accepted")
    print("packaged per-user embedding server proof self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--checksum-file", type=Path)
    parser.add_argument("--expected-version")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--plugin-root", type=Path)
    parser.add_argument("--out-dir", type=Path, default=Path("target/packaged-agent-proof"))
    parser.add_argument("--query", default=DEFAULT_QUERY)
    parser.add_argument("--question", default=DEFAULT_QUESTION)
    parser.add_argument("--additional-project", type=Path, action="append", default=[])
    parser.add_argument("--additional-query", action="append", default=[])
    parser.add_argument("--timeout-secs", type=int, default=900)
    parser.add_argument("--version-only", action="store_true")
    parser.add_argument(
        "--proof-tier",
        choices=("calibration", "hosted_package", "protected_hardware", "installed_runtime"),
        default="hosted_package",
    )
    parser.add_argument("--plugin-handoff", action="store_true")
    parser.add_argument("--engine-policy", choices=("accelerated", "cpu_explicit"))
    parser.add_argument("--expected-backend")
    parser.add_argument("--qualification-matrix-cell")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--qualification-evidence", type=Path)
    parser.add_argument("--produce-qualification-evidence", action="store_true")
    parser.add_argument("--server-behavior-only", action="store_true")
    parser.add_argument("--publication-fault-evidence", type=Path)
    parser.add_argument("--retrieval-quality-evidence", type=Path)
    parser.add_argument("--calibration-bundle", type=Path)
    parser.add_argument("--enforce-calibration-freeze-lineage", action="store_true")
    parser.add_argument("--calibration-run-index", type=int)
    parser.add_argument("--calibration-run-output", type=Path)
    parser.add_argument("--assemble-calibration-bundle", action="store_true")
    parser.add_argument("--calibration-run", type=Path, action="append", default=[])
    parser.add_argument("--calibration-bundle-output", type=Path)
    parser.add_argument("--frozen-constant-set-output", type=Path)
    parser.add_argument("--freeze-selected-at")
    parser.add_argument("--calibration-producer-repository")
    parser.add_argument("--calibration-producer-workflow-path")
    parser.add_argument("--calibration-producer-run-id")
    parser.add_argument("--calibration-producer-run-attempt")
    parser.add_argument("--calibration-producer-artifact")
    parser.add_argument("--installed-plugin-provenance", type=Path)
    parser.add_argument("--installed-plugin-data", type=Path)
    parser.add_argument(
        "--installed-plugin-source",
        choices=("marketplace", "candidate"),
        default="marketplace",
    )
    parser.add_argument("--prepare-candidate-installed-proof", action="store_true")
    parser.add_argument("--candidate-plugin-root-output", type=Path)
    parser.add_argument("--candidate-plugin-data-output", type=Path)
    parser.add_argument("--installed-plugin-provenance-output", type=Path)
    parser.add_argument("--candidate-producer-repository")
    parser.add_argument("--candidate-producer-workflow-path")
    parser.add_argument("--candidate-producer-run-id")
    parser.add_argument("--candidate-producer-run-attempt")
    parser.add_argument("--candidate-artifact-name")
    parser.add_argument("--measurement-protocol", type=Path, default=MEASUREMENT_PROTOCOL)
    parser.add_argument("--expected-source-sha")
    parser.add_argument("--expected-source-tree")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    args.measurement_protocol = args.measurement_protocol.resolve()
    if args.assemble_calibration_bundle:
        result = assemble_calibration_bundle(args)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    if args.prepare_candidate_installed_proof:
        result = prepare_candidate_installed_proof(args)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    require(args.archive and args.checksum_file and args.expected_version, "archive, checksum, and expected version are required")
    args.archive = args.archive.resolve()
    args.checksum_file = args.checksum_file.resolve()
    args.out_dir = args.out_dir.resolve()
    if args.qualification_evidence is not None:
        args.qualification_evidence = args.qualification_evidence.resolve()
    if args.publication_fault_evidence is not None:
        args.publication_fault_evidence = args.publication_fault_evidence.resolve()
    if args.retrieval_quality_evidence is not None:
        args.retrieval_quality_evidence = args.retrieval_quality_evidence.resolve()
    if args.calibration_bundle is not None:
        args.calibration_bundle = args.calibration_bundle.resolve()
    if args.calibration_run_output is not None:
        args.calibration_run_output = args.calibration_run_output.resolve()
    require(
        (args.calibration_run_output is None)
        == (args.calibration_run_index is None),
        "--calibration-run-output and --calibration-run-index must be supplied together",
    )
    require(
        args.calibration_run_output is None or args.proof_tier == "calibration",
        "calibration run output is valid only for the calibration proof tier",
    )
    if args.installed_plugin_provenance is not None:
        args.installed_plugin_provenance = args.installed_plugin_provenance.resolve()
    if args.installed_plugin_data is not None:
        args.installed_plugin_data = args.installed_plugin_data.resolve()
    if args.server_behavior_only:
        require(
            not args.version_only and args.proof_tier != "calibration",
            "server-behavior-only proof requires a frozen non-calibration runtime tier",
        )
        require(
            not args.produce_qualification_evidence
            and args.qualification_evidence is None
            and args.retrieval_quality_evidence is None
            and args.publication_fault_evidence is None,
            "server-behavior-only proof rejects qualification and retrieval-quality inputs",
        )
    args.out_dir.mkdir(parents=True, exist_ok=True)
    require(sha256(args.archive) == expected_archive_digest(args.checksum_file, args.archive), "archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-") as raw:
        root = Path(raw)
        unpack_archive(args.archive, root / "unpacked")
        cli = find_cli(root / "unpacked")
        manifest = load_native_manifest(root / "unpacked", cli, args.expected_version)
        if args.expected_source_sha:
            require(
                manifest["source"]["commit"] == args.expected_source_sha,
                "package source commit does not match --expected-source-sha",
            )
        if args.expected_source_tree:
            require(
                manifest["source"]["tree"] == args.expected_source_tree,
                "package source tree does not match --expected-source-tree",
            )
        require_frozen = not args.version_only and args.proof_tier != "calibration"
        require(
            not args.enforce_calibration_freeze_lineage or require_frozen,
            "calibration freeze lineage is valid only for the immediate frozen proof",
        )
        measurement_contract = verify_package_server_contracts(
            manifest,
            args.measurement_protocol,
            require_frozen=require_frozen,
        )
        calibration_bundle = None
        if require_frozen:
            require(
                args.calibration_bundle is not None,
                f"{args.proof_tier} proof requires --calibration-bundle for the frozen constant set",
            )
            require(
                args.calibration_producer_run_id is not None
                and args.calibration_producer_artifact is not None,
                f"{args.proof_tier} proof requires authenticated calibration producer run and artifact identity",
            )
            calibration_bundle = verify_calibration_bundle(
                args.calibration_bundle,
                measurement_contract,
                frozen_source=manifest["source"],
                repository_root=Path(__file__).resolve().parents[2],
                enforce_source_lineage=args.enforce_calibration_freeze_lineage,
                expected_producer_run_id=args.calibration_producer_run_id,
                expected_producer_artifact=args.calibration_producer_artifact,
            )
        env = isolated_environment(root, args.engine_policy, args.offline)
        version = run([str(cli), "--version"], env=env, cwd=root, timeout=args.timeout_secs)
        require(args.expected_version in version["stdout"], f"CLI version does not contain {args.expected_version}")
        help_result = run([str(cli), "--help"], env=env, cwd=root, timeout=args.timeout_secs)
        help_text = help_result["stdout"].lower()
        require(
            not any(token in help_text for token in LEGACY_HELP_TOKENS),
            "top-level help exposes deleted embedding lifecycle terminology",
        )
        summary: dict[str, object] = {
            "version": version,
            "help": help_result,
            "package_contract": {
                "manifest": manifest,
                "answer_quality_claim": False,
                "release_readiness_claim": False,
                "measurement_contract": measurement_contract,
                "calibration_bundle": calibration_bundle,
                "claim_scope": (
                    "server_behavior_only"
                    if args.server_behavior_only
                    else "qualification"
                ),
                "highest_proof_tier": "package_structure",
            },
        }
        if not args.version_only:
            require(args.project is not None, "--project is required for the runtime proof")
            require(args.engine_policy is not None, "--engine-policy is required for the runtime proof")
            runtime = prove_runtime(args, cli, env, root, args.out_dir, manifest)
            qualification_cli = Path(
                require_nonempty_string(
                    runtime.get("_qualification_cli_path"),
                    "runtime qualification executable",
                )
            )
            if args.produce_qualification_evidence:
                produce_qualification_evidence(
                    args,
                    qualification_cli,
                    env,
                    root,
                    runtime,
                    manifest,
                    sha256(args.archive),
                    measurement_contract,
                )
            runtime.pop("_qualification_cli_path", None)
            runtime.pop("_qualification_projects", None)
            runtime.pop("_memory_observations", None)
            summary["runtime"] = runtime
            summary["package_contract"]["runtime_evidence"] = verify_runtime_against_manifest(
                manifest, runtime, args.engine_policy
            )
            summary["package_contract"]["highest_proof_tier"] = (
                "calibration" if args.proof_tier == "calibration" else "hosted_package"
            )
            if args.server_behavior_only:
                summary["server_behavior"] = {
                    "status": "pass",
                    "runtime_tier_exercised": args.proof_tier,
                    "answer_quality_claim": False,
                    "retrieval_quality_claim": False,
                    "release_readiness_claim": False,
                    "installed_runtime_provenance_proven": (
                        args.proof_tier == "installed_runtime"
                        and isinstance(runtime.get("installed_plugin"), dict)
                        and isinstance(runtime.get("managed_runtime"), dict)
                    ),
                }
                summary["package_contract"]["highest_proof_tier"] = (
                    "server_behavior"
                )
            elif args.proof_tier != "calibration":
                require(
                    args.qualification_evidence is not None,
                    f"{args.proof_tier} proof requires --qualification-evidence from the exact live scenario run",
                )
                try:
                    retained = json.loads(args.qualification_evidence.read_text(encoding="utf-8"))
                except json.JSONDecodeError as exc:
                    raise ProofFailure(f"qualification evidence is not valid JSON: {exc}") from exc
                require(isinstance(retained, dict), "qualification evidence must be an object")
                requested_matrix_cell_id = require_nonempty_string(
                    args.qualification_matrix_cell,
                    f"{args.proof_tier} proof requires --qualification-matrix-cell",
                )
                requested_backend = args.expected_backend or require_nonempty_string(
                    runtime["identity"].get("embedding_backend"),
                    "runtime embedding backend",
                )
                requested_matrix_cell = selected_qualification_matrix_cell(
                    measurement_contract["measurement_protocol"],
                    cell_id=requested_matrix_cell_id,
                    target=manifest["asset_target"],
                    proof_tier=args.proof_tier,
                    expected_policy=args.engine_policy,
                    expected_backend=requested_backend,
                )
                summary["qualification"] = verify_retained_qualification(
                    retained,
                    manifest=manifest,
                    archive_sha256=sha256(args.archive),
                    shared_identity=runtime["shared_identity"],
                    measurement_contract=measurement_contract,
                    required_tier=args.proof_tier,
                    required_matrix_cell_id=requested_matrix_cell_id,
                    expected_policy=args.engine_policy,
                    expected_backend=requested_backend,
                    expected_accelerator_claim=requested_matrix_cell[
                        "accelerator_claim"
                    ],
                    installed_plugin=runtime.get("installed_plugin"),
                    managed_runtime=runtime.get("managed_runtime"),
                )
                summary["package_contract"]["highest_proof_tier"] = args.proof_tier
            elif args.qualification_evidence is not None and args.qualification_evidence.is_file():
                try:
                    calibration = json.loads(
                        args.qualification_evidence.read_text(encoding="utf-8")
                    )
                except json.JSONDecodeError as exc:
                    raise ProofFailure(
                        f"calibration evidence is not valid JSON: {exc}"
                    ) from exc
                require(
                    isinstance(calibration, dict)
                    and calibration.get("schema_version") == 1
                    and calibration.get("status") == "calibration"
                    and calibration.get("tier") == "calibration",
                    "calibration evidence has the wrong schema, status, or tier",
                )
                summary["qualification"] = calibration
        write_json(args.out_dir / "summary.json", summary)
    print(f"packaged CodeStory {args.proof_tier} proof passed: {args.out_dir / 'summary.json'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ProofFailure, subprocess.TimeoutExpired, OSError, json.JSONDecodeError) as exc:
        print(f"packaged CodeStory proof failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
