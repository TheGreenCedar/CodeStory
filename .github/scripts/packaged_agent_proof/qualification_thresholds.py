"""Frozen qualification threshold selection by protected-hardware matrix cell."""

from __future__ import annotations

from .contract_primitives import require_positive_int
from .foundation import require


WINDOWS_VULKAN_MATRIX_CELL = "protected_windows_x64_vulkan"
WINDOWS_SPAWN_METRIC = "spawn_convergence"
WINDOWS_WARM_CONNECT_METRIC = "existing_owner_connect"


def _positive_threshold(value: object, field: str) -> int | float:
    require(
        isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0,
        f"{field} must be a positive number",
    )
    return value


def windows_warm_connect_probe_rule(protocol: dict) -> dict:
    probes = protocol.get("qualification_threshold_override_probes")
    require(
        isinstance(probes, dict)
        and set(probes) == {WINDOWS_VULKAN_MATRIX_CELL}
        and isinstance(probes[WINDOWS_VULKAN_MATRIX_CELL], dict)
        and set(probes[WINDOWS_VULKAN_MATRIX_CELL])
        == {WINDOWS_WARM_CONNECT_METRIC},
        "qualification threshold override probes changed shape",
    )
    rule = probes[WINDOWS_VULKAN_MATRIX_CELL][WINDOWS_WARM_CONNECT_METRIC]
    require(
        isinstance(rule, dict)
        and set(rule)
        == {
            "aggregation",
            "connect_poll_interval_ms",
            "maximum_probe_duration_ms",
            "poll_intervals",
            "qualification_samples_are_selection_inputs",
            "sample_count",
        },
        "Windows resident-owner warm-connect probe rule changed shape",
    )
    poll_interval_ms = require_positive_int(
        rule.get("connect_poll_interval_ms"),
        "Windows warm-connect probe connect poll interval",
    )
    poll_intervals = require_positive_int(
        rule.get("poll_intervals"),
        "Windows warm-connect probe admitted poll intervals",
    )
    require(
        rule.get("aggregation") == "maximum"
        and require_positive_int(
            rule.get("sample_count"),
            "Windows warm-connect probe sample count",
        )
        == 30
        and require_positive_int(
            rule.get("maximum_probe_duration_ms"),
            "Windows warm-connect probe maximum duration",
        )
        == 90_000
        and rule.get("qualification_samples_are_selection_inputs") is False,
        "Windows resident-owner warm-connect probe rule is not preregistered",
    )
    return {
        **rule,
        "selected_threshold_ms": poll_interval_ms * poll_intervals,
    }


def qualification_metric_sample_policy(
    protocol: dict,
    metric: str,
    matrix_cell_id: str,
) -> dict:
    policy = protocol["metric_sampling"][metric]
    if (
        matrix_cell_id == WINDOWS_VULKAN_MATRIX_CELL
        and metric == WINDOWS_WARM_CONNECT_METRIC
    ):
        rule = windows_warm_connect_probe_rule(protocol)
        return {
            "sample_count": rule["sample_count"],
            "aggregation": rule["aggregation"],
        }
    return policy


def verify_qualification_threshold_contract(
    constant_set: dict,
    required_metrics: set[str],
    protocol: dict,
) -> None:
    thresholds = constant_set.get("qualification_thresholds")
    require(
        isinstance(thresholds, dict) and set(thresholds) == required_metrics,
        "embedding server qualification thresholds do not match the measurement metrics",
    )
    for metric, threshold in thresholds.items():
        _positive_threshold(threshold, f"qualification threshold {metric}")

    overrides = constant_set.get("qualification_threshold_overrides")
    require(
        isinstance(overrides, dict)
        and set(overrides) == {WINDOWS_VULKAN_MATRIX_CELL}
        and isinstance(overrides[WINDOWS_VULKAN_MATRIX_CELL], dict)
        and set(overrides[WINDOWS_VULKAN_MATRIX_CELL])
        == {WINDOWS_SPAWN_METRIC, WINDOWS_WARM_CONNECT_METRIC},
        "embedding server qualification threshold overrides changed shape",
    )
    windows_spawn = _positive_threshold(
        overrides[WINDOWS_VULKAN_MATRIX_CELL][WINDOWS_SPAWN_METRIC],
        "Windows Vulkan spawn-convergence threshold",
    )
    selected_values = constant_set.get(
        "calibration_required_values"
        if constant_set.get("status") == "frozen"
        else "draft_values"
    )
    require(
        isinstance(selected_values, dict)
        and windows_spawn
        == require_positive_int(
            selected_values.get("connect_timeout_ms"),
            "selected connect timeout",
        ),
        "Windows Vulkan spawn-convergence threshold must equal the selected slow-host connect bound",
    )
    windows_warm_connect = _positive_threshold(
        overrides[WINDOWS_VULKAN_MATRIX_CELL][WINDOWS_WARM_CONNECT_METRIC],
        "Windows Vulkan existing-owner-connect threshold",
    )
    probe_rule = windows_warm_connect_probe_rule(protocol)
    require(
        windows_warm_connect == probe_rule["selected_threshold_ms"],
        "Windows Vulkan existing-owner-connect threshold must come from the declared native probe rule",
    )


def qualification_threshold_for(
    constant_set: dict,
    metric: str,
    matrix_cell_id: str,
) -> int | float:
    thresholds = constant_set["qualification_thresholds"]
    overrides = constant_set["qualification_threshold_overrides"]
    return overrides.get(matrix_cell_id, {}).get(metric, thresholds[metric])
