"""Calibration metric aggregation and constant selection."""

from __future__ import annotations

import math

from .calibration_records import (
    CalibrationAccumulator,
    CalibrationBundle,
    CalibrationRun,
    _calibration_accumulator,
    _calibration_run,
    _calibration_sample,
    _record_calibration_durations,
)
from .contract_primitives import require_exact_keys, require_nonnegative_int, require_positive_int
from .foundation import require


def _calibration_metric_value(
    metric: str,
    record: object,
    *,
    run: CalibrationRun,
    bundle: CalibrationBundle,
    accumulator: CalibrationAccumulator,
    maximum_suspend_ns: int,
) -> float | int:
    field = f"calibration run {run.position} metric {metric}"
    require(isinstance(record, dict), f"{field} is malformed")
    require_exact_keys(record, {"unit", "samples"}, field)
    require(
        record["unit"] == bundle.protocol["metric_contracts"][metric]["unit"],
        f"{field} used the wrong unit",
    )
    samples = record["samples"]
    policy = bundle.protocol["calibration_metric_sampling"][metric]
    require(
        isinstance(samples, list)
        and len(samples) == policy["sample_count_per_run"] == 1,
        f"{field} sample count changed",
    )
    normalized = [
        _calibration_sample(
            sample,
            metric=metric,
            index=index,
            run=run,
            bundle=bundle,
            accumulator=accumulator,
            maximum_suspend_ns=maximum_suspend_ns,
        )
        for index, sample in enumerate(samples)
    ]
    for sample in normalized:
        _record_calibration_durations(
            metric,
            sample,
            field=field,
            accumulator=accumulator,
        )
    return normalized[0].value


def _verified_calibration_runs(
    bundle: CalibrationBundle,
) -> CalibrationAccumulator:
    accumulator = _calibration_accumulator(bundle)
    suspend_contract = bundle.protocol["clock_policy"]["suspend_detection"]
    maximum_suspend_ns = require_nonnegative_int(
        suspend_contract["maximum_inclusive_minus_awake_ns"],
        "calibration suspend-detection tolerance",
    )
    for position, raw_run in enumerate(bundle.runs):
        run = _calibration_run(
            raw_run,
            position=position,
            bundle=bundle,
            accumulator=accumulator,
        )
        for metric in sorted(run.metrics):
            accumulator.metric_values[metric].append(
                _calibration_metric_value(
                    metric,
                    run.metrics[metric],
                    run=run,
                    bundle=bundle,
                    accumulator=accumulator,
                    maximum_suspend_ns=maximum_suspend_ns,
                )
            )
    require(
        accumulator.observed_run_cells == accumulator.expected_run_cells,
        "calibration bundle does not exactly cover every matrix cell three times",
    )
    require(
        set(accumulator.server_identities_by_run) == accumulator.expected_run_cells
        and set(accumulator.materialization_reused_by_run)
        == accumulator.expected_run_cells
        and all(
            len(identities) == 1
            for identities in accumulator.server_identities_by_run.values()
        ),
        "each calibration run must use exactly one fresh server generation",
    )
    run_identities = [
        next(iter(accumulator.server_identities_by_run[run_cell]))
        for run_cell in sorted(accumulator.expected_run_cells)
    ]
    require(
        len(set(run_identities)) == len(run_identities),
        "calibration clean runs reused a server generation",
    )
    packages = accumulator.packages_by_cell.values()
    require(
        len({package["release_version"] for package in packages}) == 1
        and len({package["model_sha256"] for package in packages}) == 1,
        "calibration matrix cells did not use one release version and model",
    )
    require(
        all(accumulator.duration_values_ms.values()),
        "calibration bundle omitted a production-constant raw source cell",
    )
    return accumulator


def _selected_calibration_constants(
    durations: dict[str, list[float]],
    constant_selection: dict,
) -> dict:
    formulas = constant_selection["formulas"]
    connect_floor = require_positive_int(
        formulas["connect_timeout_ms"]["slow_host_floor_ms"],
        "connect timeout slow-host floor",
    )
    spawn_floor = require_positive_int(
        formulas["spawn_convergence_timeout_ms"]["slow_host_floor_ms"],
        "spawn convergence slow-host floor",
    )
    query_floor = require_positive_int(
        formulas["request_deadlines_ms"]["query_request_deadline_ms"][
            "slow_host_floor_ms"
        ],
        "query request slow-host floor",
    )
    connect = max(
        connect_floor,
        math.ceil(max(durations["existing_owner_connect_duration"]) * 1.50),
    )
    spawn = max(
        spawn_floor,
        math.ceil(max(durations["spawn_convergence_duration"]) * 1.50),
    )
    query = max(
        query_floor,
        math.ceil(max(durations["query_request_duration"]) * 1.50),
    )
    replay_floor = require_positive_int(
        formulas["request_deadlines_ms"]["bulk_request_deadline_ms"][
            "replay_success_budget_slow_host_floor_ms"
        ],
        "bulk replay success slow-host floor",
    )
    retry_floor = require_positive_int(
        formulas["capacity_retry_policy"]["retry_after_slow_host_floor_ms"],
        "capacity retry slow-host floor",
    )
    initial_floor = require_positive_int(
        formulas["election_backoff_policy"]["initial_backoff_slow_host_floor_ms"],
        "election initial-backoff slow-host floor",
    )
    maximum_floor = require_positive_int(
        formulas["election_backoff_policy"]["maximum_backoff_slow_host_floor_ms"],
        "election maximum-backoff slow-host floor",
    )
    hard_floor = require_positive_int(
        formulas["hard_native_no_progress_ms"]["slow_host_floor_ms"],
        "native no-progress slow-host floor",
    )
    cadence_floor = require_positive_int(
        formulas["watchdog_cadence_ms"]["slow_host_floor_ms"],
        "watchdog cadence slow-host floor",
    )
    replay = max(
        replay_floor,
        query,
        math.ceil(max(durations["bulk_request_duration"]) * 1.50),
    )
    retry = max(
        retry_floor,
        math.floor(min(durations["capacity_condition_duration"]) * 0.50),
    )
    initial = max(
        initial_floor,
        math.ceil(max(durations["existing_owner_connect_duration"]) * 0.50),
    )
    maximum = max(
        maximum_floor,
        initial,
        math.ceil(max(durations["spawn_convergence_duration"]) * 0.25),
    )
    hard = max(
        hard_floor,
        math.ceil(max(durations["successful_operation_duration"]) * 4.00),
    )
    cadence = max(cadence_floor, math.floor(hard / 20))
    return {
        "connect_timeout_ms": connect,
        "spawn_convergence_timeout_ms": spawn,
        "request_deadlines_ms": {
            "query_request_deadline_ms": query,
            "bulk_replay_success_budget_ms": replay,
            "bulk_request_deadline_ms": hard + cadence + spawn + replay,
        },
        "capacity_retry_policy": {
            "retry_after_ms": retry,
            "retry_class": "after_capacity_change",
            "retry_condition_source": "named_condition_from_typed_capacity_response",
        },
        "election_backoff_policy": {
            "initial_backoff_ms": initial,
            "maximum_backoff_ms": maximum,
            "jitter": (
                "sha256(process_start_id||attempt) modulo inclusive "
                "[initial_backoff_ms,maximum_backoff_ms]"
            ),
        },
        "hard_native_no_progress_ms": hard,
        "watchdog_cadence_ms": cadence,
    }
