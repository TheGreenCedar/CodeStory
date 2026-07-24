"""Measurement protocol and holdout-contract loading."""

from __future__ import annotations

import json
import re
from pathlib import Path

from .foundation import (
    HOLDOUT_TASK_ROOT,
    LOWER_TIER_NONCLAIMS,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    QUALIFICATION_SCHEMA_VERSION,
    REPOSITORY_ROOT,
    REQUIRED_HOLDOUT_TASK_FILES,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    SERVER_CONSTANT_SET,
    SERVER_PROTOCOL,
    ProofFailure,
    require,
)
from .contract_primitives import (
    canonical_sha256,
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
    sha256,
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
    require(isinstance(matrix, dict), "measurement protocol omitted its host/package matrix")
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


_EXPECTED_CONSTANT_ORDER = [
        "connect_timeout_ms",
        "spawn_convergence_timeout_ms",
        "hard_native_no_progress_ms",
        "watchdog_cadence_ms",
        "request_deadlines_ms",
        "capacity_retry_policy",
        "election_backoff_policy",
    ]


def _verify_constant_sources(constant_selection: dict) -> None:
    require(
        constant_selection["selection_order"] == _EXPECTED_CONSTANT_ORDER,
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


def _verify_constant_formulas(constant_selection: dict) -> None:
    formulas = constant_selection["formulas"]
    require(
        isinstance(formulas, dict)
        and set(formulas) == set(_EXPECTED_CONSTANT_ORDER),
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


def _verify_constant_selection(protocol: dict) -> None:
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
    _verify_constant_sources(constant_selection)
    _verify_constant_formulas(constant_selection)


def _verify_thresholds_and_clock(protocol: dict) -> None:
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


def load_measurement_protocol(path: Path) -> tuple[dict, str]:
    protocol = _measurement_document(path)
    required_metrics, metric_contracts = _verify_scenario_and_metric_contracts(
        protocol
    )
    _verify_measurement_matrices(protocol)
    _verify_measurement_sampling(protocol, required_metrics, metric_contracts)
    _verify_constant_selection(protocol)
    _verify_thresholds_and_clock(protocol)
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
