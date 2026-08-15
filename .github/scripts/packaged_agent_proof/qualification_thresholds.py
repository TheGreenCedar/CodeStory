"""Frozen qualification threshold selection by protected-hardware matrix cell."""

from __future__ import annotations

from .contract_primitives import require_positive_int
from .foundation import require


WINDOWS_VULKAN_MATRIX_CELL = "protected_windows_x64_vulkan"
WINDOWS_SPAWN_METRIC = "spawn_convergence"


def _positive_threshold(value: object, field: str) -> int | float:
    require(
        isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0,
        f"{field} must be a positive number",
    )
    return value


def verify_qualification_threshold_contract(
    constant_set: dict,
    required_metrics: set[str],
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
        and set(overrides[WINDOWS_VULKAN_MATRIX_CELL]) == {WINDOWS_SPAWN_METRIC},
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


def qualification_threshold_for(
    constant_set: dict,
    metric: str,
    matrix_cell_id: str,
) -> int | float:
    thresholds = constant_set["qualification_thresholds"]
    overrides = constant_set["qualification_threshold_overrides"]
    return overrides.get(matrix_cell_id, {}).get(metric, thresholds[metric])
