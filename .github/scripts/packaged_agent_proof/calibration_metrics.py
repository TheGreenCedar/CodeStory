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
from .contracts import require_exact_keys, require_nonnegative_int
from .foundation import require

def _aggregate_calibration_values(
    aggregation: str,
    values: list[float | int],
) -> float | int:
    if aggregation == "maximum":
        return max(values)
    if aggregation == "minimum":
        return min(values)
    if aggregation == "exact":
        return values[0]
    require(
        aggregation == "all_rows_pass_rate",
        f"unknown calibration aggregation {aggregation}",
    )
    return sum(values) / len(values)


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
    policy = bundle.protocol["metric_sampling"][metric]
    require(
        isinstance(samples, list) and len(samples) == policy["sample_count"],
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
    identities = [sample.identity for sample in normalized]
    if policy.get("independence") == "distinct_server_instance_per_sample":
        require(
            len({identity[:2] for identity in identities}) == len(normalized),
            f"{field} samples are not independent",
        )
    else:
        require(len(set(identities)) == 1, f"{field} changed server identity")
    for sample in normalized:
        _record_calibration_durations(
            metric,
            sample,
            field=field,
            accumulator=accumulator,
        )
    return _aggregate_calibration_values(
        policy["aggregation"],
        [sample.value for sample in normalized],
    )


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


def _selected_calibration_constants(durations: dict[str, list[float]]) -> dict:
    connect = max(1, math.ceil(max(durations["existing_owner_connect_duration"]) * 1.50))
    spawn = max(1, math.ceil(max(durations["spawn_convergence_duration"]) * 1.50))
    query = max(1, math.ceil(max(durations["query_request_duration"]) * 1.50))
    replay = max(query, math.ceil(max(durations["bulk_request_duration"]) * 1.50))
    retry = max(1, math.floor(min(durations["capacity_condition_duration"]) * 0.50))
    initial = max(1, math.ceil(max(durations["existing_owner_connect_duration"]) * 0.50))
    maximum = max(initial, math.ceil(max(durations["spawn_convergence_duration"]) * 0.25))
    hard = max(1, math.ceil(max(durations["successful_operation_duration"]) * 4.00))
    cadence = max(1, math.floor(hard / 20))
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


def _selected_calibration_thresholds(
    values_by_metric: dict[str, list[float | int]],
    metric_contracts: dict,
) -> dict[str, float | int]:
    thresholds: dict[str, float | int] = {}
    for metric, values in values_by_metric.items():
        comparison = metric_contracts[metric]["comparison"]
        if comparison == "less_than_or_equal":
            threshold: float | int = math.ceil(max(values) * 1.20)
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
    return thresholds
