"""Compiled constant-selection and threshold contract validation."""

from __future__ import annotations

from .contract_primitives import require_exact_keys
from .foundation import require

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
            and cell.get("artifact") == "constant-calibration-run-*.raw.json"
            and isinstance(cell.get("operand"), str)
            and bool(cell["operand"])
            and (
                isinstance(cell.get("metric"), str)
                or isinstance(cell.get("metrics"), list)
            )
            for cell in source_cells.values()
        ),
        "production constants do not name their exact raw source cells",
    )
    require(
        constant_selection["clean_run_requirements"]
        == {
            "minimum_runs_per_matrix_cell": 3,
            "matrix_coverage": "every_required_calibration_matrix_cell",
            "source_identity": "one_exact_candidate_commit_and_tree",
            "artifact_selection": "all_preregistered_clean_runs",
            "fresh_server_identity": "disjoint_across_clean_runs",
            "unplanned_suspend": False,
            "outlier_removal": "none",
        },
        "production-constant calibration run selection changed",
    )


def _verify_constant_formulas(constant_selection: dict) -> None:
    formulas = constant_selection["formulas"]
    require(
        isinstance(formulas, dict) and set(formulas) == set(_EXPECTED_CONSTANT_ORDER),
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
        formulas["connect_timeout_ms"].get("formula")
        == "max(2000,ceiling(maximum_raw_value_ms_across_all_selected_samples*1.50))"
        and formulas["connect_timeout_ms"].get("slow_host_floor_ms") == 2000
        and formulas["spawn_convergence_timeout_ms"].get("formula")
        == "max(15000,ceiling(maximum_raw_value_ms_across_all_selected_samples*1.50))"
        and formulas["spawn_convergence_timeout_ms"].get("slow_host_floor_ms")
        == 15000,
        "connect or spawn slow-host floor changed",
    )
    require(
        formulas["request_deadlines_ms"]
        .get("query_request_deadline_ms", {})
        .get("formula")
        == "max(10000,ceiling(maximum_raw_value_ms_across_all_selected_samples*1.50))"
        and formulas["request_deadlines_ms"]
        .get("query_request_deadline_ms", {})
        .get("slow_host_floor_ms")
        == 10000
        and formulas["request_deadlines_ms"]
        .get("bulk_request_deadline_ms", {})
        .get("replay_success_budget_formula")
        == "max(144537,query_request_deadline_ms,ceiling(maximum_raw_value_ms_across_all_selected_samples*1.50))"
        and formulas["request_deadlines_ms"]
        .get("bulk_request_deadline_ms", {})
        .get("replay_success_budget_slow_host_floor_ms")
        == 144537
        and formulas["request_deadlines_ms"]
        .get("bulk_request_deadline_ms", {})
        .get("formula")
        == "hard_native_no_progress_ms+watchdog_cadence_ms+spawn_convergence_timeout_ms+bulk_replay_success_budget_ms",
        "request-deadline selection formulas changed",
    )
    require(
        formulas["capacity_retry_policy"].get("retry_after_ms_formula")
        == "max(40,floor(minimum_raw_value_ms_across_all_selected_samples*0.50))"
        and formulas["capacity_retry_policy"].get("retry_after_slow_host_floor_ms")
        == 40
        and formulas["capacity_retry_policy"].get("retry_class")
        == "after_capacity_change"
        and formulas["capacity_retry_policy"].get("retry_condition_source")
        == "named_condition_from_typed_capacity_response",
        "capacity retry selection formula or typed policy changed",
    )
    require(
        formulas["election_backoff_policy"].get("initial_backoff_ms_formula")
        == "max(7,ceiling(maximum_existing_owner_connect_duration_ms_across_all_selected_samples*0.50))"
        and formulas["election_backoff_policy"].get("initial_backoff_slow_host_floor_ms")
        == 7
        and formulas["election_backoff_policy"].get("maximum_backoff_ms_formula")
        == "max(102,initial_backoff_ms,ceiling(maximum_spawn_convergence_duration_ms_across_all_selected_samples*0.25))"
        and formulas["election_backoff_policy"].get("maximum_backoff_slow_host_floor_ms")
        == 102
        and formulas["election_backoff_policy"].get("jitter")
        == "sha256(process_start_id||attempt) modulo inclusive [initial_backoff_ms,maximum_backoff_ms]",
        "election backoff selection formula changed",
    )
    require(
        formulas["hard_native_no_progress_ms"].get("formula")
        == "max(385431,ceiling(maximum_complete_successful_operation_duration_ms_across_all_selected_samples*4.00))"
        and formulas["hard_native_no_progress_ms"].get("slow_host_floor_ms") == 385431
        and formulas["watchdog_cadence_ms"].get("formula")
        == "max(19271,floor(hard_native_no_progress_ms/20))"
        and formulas["watchdog_cadence_ms"].get("slow_host_floor_ms") == 19271,
        "native no-progress or watchdog slow-host floor changed",
    )
    require(
        constant_selection["post_result_formula_changes"] is False,
        "production constants allow post-result formula changes",
    )
    require(
        isinstance(constant_selection["slow_host_floor_rationale"], str)
        and bool(constant_selection["slow_host_floor_rationale"].strip()),
        "production constants omitted the slow-host floor rationale",
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
            "slow_host_floor_rationale",
        },
        "measurement production-constant selection",
    )
    _verify_constant_sources(constant_selection)
    _verify_constant_formulas(constant_selection)


def _verify_thresholds_and_clock(protocol: dict) -> None:
    threshold_contract = protocol.get("qualification_threshold_contract")
    require(
        isinstance(threshold_contract, dict)
        and threshold_contract
        == {
            "source": "checked_in_frozen_candidate_contract",
            "selected_by_calibration": False,
            "omitted_measurements": "preserve_checked_in_thresholds",
            "true_idle_exit": {
                "idle_timeout_ms": 60_000,
                "observation_grace_ms": 2_500,
                "formula": "idle_timeout_ms+observation_grace_ms",
                "required_threshold_ms": 62_500,
                "qualification_runs_per_available_gpu_platform": 1,
            },
        },
        "qualification-threshold contract is incomplete or mutable",
    )
    require(
        protocol.get("calibration_bundle_contract")
        == {
            "schema_version": 1,
            "required_for_frozen_qualification": True,
            "matrix_cells": "exactly_every_calibration_matrix_cell",
            "independent_clean_runs_per_matrix_cell": 3,
            "samples_per_metric_per_run": 1,
            "source_identity": "one_exact_candidate_commit_and_tree",
            "producer_identity": (
                "trusted_packaged_platform_pr_workflow_run_and_exact_artifact"
            ),
            "contract_identity": [
                "protocol_sha256",
                "measurement_protocol_sha256",
            ],
            "raw_artifact": "nine_constant_source_metrics_with_canonical_sha256",
            "clock_witnesses": "awake_monotonic_plus_suspend_inclusive_per_sample",
            "successful_operation_operand": "successful_operation_duration_ns",
            "freeze_digest_inputs": [
                "selection_protocol",
                "source",
                "producer",
                "contracts",
                "run_artifact_sha256s",
                "calibration_required_values",
            ],
            "constant_set_comparison": "exact_recomputed_runtime_constants_and_freeze_record",
            "qualification_boundary": (
                "lifecycle_fault_idle_memory_quality_accelerator_and_performance_are_frozen_candidate_qualification_only"
            ),
        },
        "measurement calibration-bundle contract is incomplete or mutable",
    )
    rules = protocol.get("measurement_rules")
    require(
        isinstance(rules, dict)
        and rules.get("calibration_and_qualification_are_distinct") is True
        and rules.get("constants_frozen_before_qualification") is True
        and rules.get("missing_required_cell_fails") is True
        and rules.get("calibration_runs_full_qualification") is False
        and rules.get("qualification_runs_once_per_available_gpu_platform") is True
        and rules.get("threshold_movement_after_results") is False,
        "calibration and qualification boundary is incomplete or mutable",
    )
    clock_policy = protocol.get("clock_policy")
    suspend = (
        clock_policy.get("suspend_detection")
        if isinstance(clock_policy, dict)
        else None
    )
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
