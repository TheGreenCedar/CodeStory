"""Qualification matrix selection and measurement sample evaluation."""

from __future__ import annotations

from .foundation import CANDIDATE_QUALIFICATION_MATRIX_ALIASES, require
from .contract_primitives import (
    normalized_backend,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_positive_int,
    require_sha256,
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


def _measurement_process_and_clock(
    metric: str,
    sample: dict,
    allowed_awake_apis: set[str],
) -> str:
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
    require_positive_int(
        process["pid"],
        f"qualification measurement {metric} process.pid",
    )
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
        clock["domain"] == "awake_monotonic"
        and clock["api"] in allowed_awake_apis,
        f"qualification measurement {metric} used an unsupported awake clock",
    )
    require_nonnegative_int(
        clock["resolution_ns"],
        f"qualification measurement {metric} clock.resolution_ns",
    )
    return require_nonempty_string(
        clock["boot_id"],
        f"qualification measurement {metric} clock.boot_id",
    )


def _measurement_interval(
    metric: str,
    sample: dict,
    boundaries: list[str],
) -> tuple[int, int]:
    points = []
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
            require_nonnegative_int(
                point["observed_ns"],
                f"qualification measurement {metric} {point_name}.observed_ns",
            )
        )
    require(
        points[1] >= points[0],
        f"qualification measurement {metric} awake clock moved backwards",
    )
    return points[0], points[1]


def _verified_awake_delta(
    metric: str,
    sample: dict,
    *,
    boundaries: list[str],
    allowed_awake_apis: set[str],
    inclusive_api: str,
    maximum_suspend_ns: int,
) -> int:
    boot_id = _measurement_process_and_clock(metric, sample, allowed_awake_apis)
    started_ns, finished_ns = _measurement_interval(metric, sample, boundaries)
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
        witness["awake_started_ns"] == started_ns
        and witness["awake_finished_ns"] == finished_ns,
        f"qualification measurement {metric} suspend witness changed phase timestamps",
    )
    require(
        witness["inclusive_clock_api"] == inclusive_api,
        f"qualification measurement {metric} used the wrong suspend-inclusive clock",
    )
    inclusive_started = require_nonnegative_int(
        witness["inclusive_started_ns"],
        f"qualification measurement {metric} suspend witness inclusive_started_ns",
    )
    inclusive_finished = require_nonnegative_int(
        witness["inclusive_finished_ns"],
        f"qualification measurement {metric} suspend witness inclusive_finished_ns",
    )
    require(
        inclusive_finished >= inclusive_started,
        f"qualification measurement {metric} suspend-inclusive clock moved backwards",
    )
    require(
        witness["boot_id_started"] == boot_id
        and witness["boot_id_finished"] == boot_id,
        f"qualification measurement {metric} crossed a boot boundary",
    )
    awake_delta = finished_ns - started_ns
    require(
        abs((inclusive_finished - inclusive_started) - awake_delta)
        <= maximum_suspend_ns,
        f"qualification measurement {metric} crossed an unplanned suspend or power transition",
    )
    return awake_delta


_SUCCESSFUL_OPERATION_METRICS = {
    "cold_first_vector",
    "first_product_ready",
    "warm_query_ipc",
    "warm_bulk_ipc",
    "bulk_documents_per_second",
    "bulk_tokens_per_second",
}

_DURATION_METRICS = {
    "existing_owner_connect",
    "spawn_convergence",
    "cold_first_vector",
    "first_product_ready",
    "warm_query_ipc",
    "warm_bulk_ipc",
    "busy_retry_usefulness",
    "true_idle_exit",
}


def _duration_metric_value(metric: str, operands: dict, awake_delta_ns: int) -> float:
    require_exact_keys(
        operands,
        ({"successful_operation_duration_ns"} if metric in _SUCCESSFUL_OPERATION_METRICS else set()),
        f"qualification measurement {metric} operands",
    )
    if metric in _SUCCESSFUL_OPERATION_METRICS:
        duration = require_nonnegative_int(
            operands["successful_operation_duration_ns"],
            f"qualification measurement {metric} successful operation duration",
        )
        require(
            duration == awake_delta_ns,
            f"qualification measurement {metric} successful operation duration differs from its awake interval",
        )
    return awake_delta_ns / 1_000_000


def _throughput_metric_value(metric: str, operands: dict, awake_delta_ns: int) -> float:
    operand = "completed_documents" if metric == "bulk_documents_per_second" else "completed_tokens"
    require_exact_keys(
        operands,
        {operand, "successful_operation_duration_ns"},
        f"qualification measurement {metric} operands",
    )
    completed = require_positive_int(
        operands[operand],
        f"qualification measurement {metric} operands.{operand}",
    )
    require(awake_delta_ns > 0, f"qualification measurement {metric} window is empty")
    require(
        require_nonnegative_int(
            operands["successful_operation_duration_ns"],
            f"qualification measurement {metric} successful operation duration",
        )
        == awake_delta_ns,
        f"qualification measurement {metric} successful operation duration differs from its awake interval",
    )
    return completed * 1_000_000_000 / awake_delta_ns


def _memory_metric_value(operands: dict) -> int:
    require_exact_keys(operands, {"processes"}, "qualification five-process memory operands")
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
    identities = set()
    roles = set()
    total = 0
    for index, observed in enumerate(processes):
        field = f"qualification memory process {index}"
        require(isinstance(observed, dict), f"{field} is malformed")
        require_exact_keys(
            observed,
            {"role", "pid", "process_start_id", "executable_sha256", "resident_bytes", "measurement_api"},
            field,
        )
        roles.add(require_nonempty_string(observed["role"], f"{field}.role"))
        pid = require_positive_int(observed["pid"], f"{field}.pid")
        start_id = require_nonempty_string(
            observed["process_start_id"],
            f"{field}.process_start_id",
        )
        identities.add((pid, start_id))
        require_sha256(observed["executable_sha256"], f"{field}.executable_sha256")
        total += require_positive_int(observed["resident_bytes"], f"{field}.resident_bytes")
        require_nonempty_string(observed["measurement_api"], f"{field}.measurement_api")
    require(
        roles == expected_roles and len(identities) == 5,
        "qualification memory evidence changed the five-process set",
    )
    return total


def _retrieval_quality_value(operands: dict) -> int:
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


def _accelerator_residency_value(
    operands: dict,
    *,
    metric: str,
    expected_policy: str,
    expected_backend: str,
) -> int:
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
        and normalized_backend(operands["backend"]) == normalized_backend(expected_backend),
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
    offloaded = require_nonnegative_int(
        operands["offloaded_layer_count"],
        "qualification accelerator offloaded layer count",
    )
    model_layers = require_positive_int(
        operands["model_layer_count"],
        "qualification accelerator model layer count",
    )
    valid = (
        expected_policy == "accelerated"
        and operands["accelerator_execution_verified"] is True
        and tensor_count > 0
        and tensor_bytes > 0
        and offloaded == model_layers
    ) or (
        expected_policy == "cpu_explicit"
        and operands["accelerator_execution_verified"] is False
        and tensor_count == 0
        and tensor_bytes == 0
        and offloaded == 0
    )
    require(
        valid,
        "qualification accelerator-residency operands contradict the selected policy",
    )
    return 1


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
    del contracts
    awake_delta = _verified_awake_delta(
        metric,
        sample,
        boundaries=phase_boundaries[metric],
        allowed_awake_apis=allowed_awake_apis,
        inclusive_api=inclusive_api,
        maximum_suspend_ns=maximum_suspend_ns,
    )
    operands = sample["operands"]
    require(
        isinstance(operands, dict),
        f"qualification measurement {metric} operands are malformed",
    )
    if metric in _DURATION_METRICS:
        return _duration_metric_value(metric, operands, awake_delta)
    if metric in {"bulk_documents_per_second", "bulk_tokens_per_second"}:
        return _throughput_metric_value(metric, operands, awake_delta)
    if metric == "total_codestory_process_memory":
        return _memory_metric_value(operands)
    if metric == "retrieval_quality":
        return _retrieval_quality_value(operands)
    require(
        metric == "backend_observed_accelerator_residency",
        f"qualification measurement verifier omitted metric {metric}",
    )
    return _accelerator_residency_value(
        operands,
        metric=metric,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
