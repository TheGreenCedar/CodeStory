"""Contracts for packaged CodeStory proof."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
from pathlib import Path

from .foundation import (
    CANDIDATE_QUALIFICATION_MATRIX_ALIASES,
    HEX_SHA256,
    HOLDOUT_TASK_ROOT,
    LOWER_TIER_NONCLAIMS,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    QUALIFICATION_SCHEMA_VERSION,
    REPOSITORY_ROOT,
    REQUIRED_HOLDOUT_TASK_FILES,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    SERVER_CONSTANT_SET,
    SERVER_LIFECYCLES,
    SERVER_PROTOCOL,
    ProofFailure,
    require,
)


def normalized_backend(value: object) -> str:
    backend = str(value or "").strip().lower()
    if backend == "mtl":
        return "metal"
    if backend.startswith("vulkan"):
        return "vulkan"
    return backend


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
                "path": path.relative_to(REPOSITORY_ROOT).as_posix(),
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


def require_opaque_identifier(value: object, field: str, *, length: int = 128) -> str:
    require(
        isinstance(value, str)
        and 1 <= len(value) <= length
        and re.fullmatch(r"[A-Za-z0-9._:-]+", value) is not None,
        f"{field} must be an opaque identifier without path or request text",
    )
    return value


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
    if cell_id in matrix:
        cell = matrix[cell_id]
    else:
        alias = CANDIDATE_QUALIFICATION_MATRIX_ALIASES.get(cell_id)
        require(alias is not None, f"unknown qualification matrix cell {cell_id!r}")
        cell = alias["cell"]
        source_cell = matrix.get(alias["source_cell_id"])
        require(
            source_cell
            == {
                **cell,
                "host_class": alias["source_host_class"],
            },
            "candidate qualification matrix alias no longer matches its frozen installed-runtime source cell",
        )
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


def validate_runtime_claim_scope(args: argparse.Namespace) -> None:
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
    if args.ground_only:
        require(
            not args.version_only and args.plugin_handoff and args.project is not None,
            "ground-only proof requires plugin handoff and one project",
        )
        require(
            not args.server_behavior_only
            and not args.produce_qualification_evidence
            and args.qualification_evidence is None
            and args.retrieval_quality_evidence is None
            and args.publication_fault_evidence is None,
            "ground-only proof rejects server, qualification, and retrieval-quality inputs",
        )
