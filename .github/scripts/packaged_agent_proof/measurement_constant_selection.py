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
