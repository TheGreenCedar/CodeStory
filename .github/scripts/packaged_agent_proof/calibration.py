"""Calibration for packaged CodeStory proof."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import secrets
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    LOWER_TIER_NONCLAIMS,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    TARGET_CONTRACTS,
    ProofFailure,
    require,
)
from .contracts import (
    assert_retained_json_privacy,
    canonical_sha256,
    load_server_measurement_contract,
    qualification_measurement_sample_value,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    selected_qualification_matrix_cell,
    sha256,
    write_json,
    write_private_json,
)
from .archive import (
    normalized_backend,
)
from .process import (
    run,
)
from .runtime import (
    metric_passes,
    produce_product_publication_fault_evidence,
    retain_five_process_memory_evidence,
    verify_fault_recovery_consistency_raw_evidence,
    verify_publication_fault_raw_evidence,
    verify_retrieval_quality_raw_evidence,
)
from .qualification import (
    qualification_artifact,
    qualification_measurement_artifact,
)

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


@dataclass(frozen=True)
class CalibrationBundle:
    path: Path
    raw: dict
    constant_set: dict
    protocol: dict
    source: dict
    producer: dict
    contracts: dict
    runs: tuple[dict, ...]
    matrix: dict


@dataclass
class CalibrationAccumulator:
    expected_run_cells: set[tuple[str, int]]
    observed_run_cells: set[tuple[str, int]]
    run_ids: set[str]
    artifact_digests: set[str]
    packages_by_cell: dict[str, dict]
    sample_ids: set[str]
    metric_values: dict[str, list[float | int]]
    duration_values_ms: dict[str, list[float]]


@dataclass(frozen=True)
class CalibrationRun:
    position: int
    matrix_cell_id: str
    matrix_cell: dict
    package: dict
    metrics: dict


@dataclass(frozen=True)
class CalibrationSample:
    identity: tuple[str, str, int]
    value: float | int
    raw: dict


def _calibration_source(value: object) -> dict:
    require(isinstance(value, dict), "calibration bundle source identity is malformed")
    require_exact_keys(value, {"commit", "tree", "tracked_dirty"}, "calibration bundle source")
    for field in ("commit", "tree"):
        require(
            isinstance(value[field], str)
            and re.fullmatch(r"[0-9a-f]{40}", value[field]) is not None,
            f"calibration bundle source.{field} is not an exact Git object id",
        )
    require(value["tracked_dirty"] is False, "calibration bundle source tree was dirty")
    return value


def _calibration_producer(
    value: object,
    source: dict,
    *,
    expected_run_id: str | None,
    expected_artifact: str | None,
) -> dict:
    require(isinstance(value, dict), "calibration bundle producer is malformed")
    require_exact_keys(
        value,
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
        value["repository"] == "TheGreenCedar/CodeStory"
        and value["workflow_path"] == ".github/workflows/packaged-platform-pr.yml"
        and isinstance(value["run_id"], str)
        and re.fullmatch(r"[1-9][0-9]*", value["run_id"]) is not None
        and isinstance(value["run_attempt"], str)
        and re.fullmatch(r"[1-9][0-9]*", value["run_attempt"]) is not None
        and value["artifact_name"] == f"embedding-calibration-bundle-{source['commit']}"
        and value["source_head_sha"] == source["commit"],
        "calibration bundle producer is not the trusted exact-head coordinator artifact",
    )
    if expected_run_id is not None or expected_artifact is not None:
        require(
            value["run_id"] == expected_run_id
            and value["artifact_name"] == expected_artifact,
            "calibration bundle producer differs from the authenticated download request",
        )
    return value


def _calibration_contracts(bundle: dict, measurement_contract: dict) -> dict:
    constant_set = measurement_contract["constant_set"]
    expected = {
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
        bundle["contracts"] == expected,
        "calibration bundle contract hashes differ from the checked-in protocols",
    )
    return bundle["contracts"]


def _calibration_bundle(
    path: Path,
    measurement_contract: dict,
    *,
    expected_producer_run_id: str | None,
    expected_producer_artifact: str | None,
) -> CalibrationBundle:
    require(
        path.is_file() and not path.is_symlink(),
        f"calibration bundle is missing or unsafe: {path}",
    )
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
    source = _calibration_source(bundle["source"])
    producer = _calibration_producer(
        bundle["producer"],
        source,
        expected_run_id=expected_producer_run_id,
        expected_artifact=expected_producer_artifact,
    )
    contracts = _calibration_contracts(bundle, measurement_contract)
    matrix = protocol["calibration_matrix"]
    runs = bundle["runs"]
    expected_count = len(matrix) * 3
    require(
        isinstance(runs, list) and len(runs) == expected_count,
        "calibration bundle must contain exactly three runs for every matrix cell",
    )
    return CalibrationBundle(
        path,
        bundle,
        constant_set,
        protocol,
        source,
        producer,
        contracts,
        tuple(runs),
        matrix,
    )


def _calibration_accumulator(bundle: CalibrationBundle) -> CalibrationAccumulator:
    metrics = set(bundle.protocol["required_metrics"]) - {"retrieval_quality"}
    return CalibrationAccumulator(
        expected_run_cells={
            (cell_id, run_index)
            for cell_id in bundle.matrix
            for run_index in range(1, 4)
        },
        observed_run_cells=set(),
        run_ids=set(),
        artifact_digests=set(),
        packages_by_cell={},
        sample_ids=set(),
        metric_values={metric: [] for metric in metrics},
        duration_values_ms={
            "existing_owner_connect_duration": [],
            "spawn_convergence_duration": [],
            "query_request_duration": [],
            "bulk_request_duration": [],
            "capacity_condition_duration": [],
            "successful_operation_duration": [],
        },
    )


def _calibration_package(
    value: object,
    *,
    position: int,
    matrix_cell_id: str,
    matrix_cell: dict,
    accumulator: CalibrationAccumulator,
) -> dict:
    field = f"calibration run {position} package"
    require(isinstance(value, dict), f"{field} is malformed")
    require_exact_keys(
        value,
        {
            "archive_sha256",
            "executable_sha256",
            "asset_target",
            "release_version",
            "model_sha256",
            "policy",
            "backend",
        },
        field,
    )
    for digest_field in ("archive_sha256", "executable_sha256", "model_sha256"):
        require_sha256(value[digest_field], f"{field}.{digest_field}")
    require(
        value["asset_target"] == matrix_cell["asset_target"]
        and value["policy"] == matrix_cell["policy"]
        and normalized_backend(value["backend"])
        == normalized_backend(matrix_cell["backend"])
        and isinstance(value["release_version"], str)
        and bool(value["release_version"]),
        f"calibration run {position} package does not match its matrix cell",
    )
    previous = accumulator.packages_by_cell.get(matrix_cell_id)
    require(
        previous is None or value == previous,
        f"calibration matrix cell {matrix_cell_id} changed package between runs",
    )
    accumulator.packages_by_cell.setdefault(matrix_cell_id, value)
    return value


def _calibration_raw_payload(
    value: object,
    *,
    position: int,
    expected_identity: dict,
    accumulator: CalibrationAccumulator,
) -> dict:
    field = f"calibration run {position} raw artifact"
    require(isinstance(value, dict), f"{field} is malformed")
    require_exact_keys(value, {"name", "sha256", "payload"}, field)
    require(
        value["name"] == "measurements.raw.json",
        f"calibration run {position} raw artifact has the wrong name",
    )
    digest = require_sha256(value["sha256"], f"{field} sha256")
    require(
        digest == canonical_sha256(value["payload"]),
        f"calibration run {position} raw artifact digest does not match its payload",
    )
    require(
        digest not in accumulator.artifact_digests,
        "calibration bundle reused one raw artifact for multiple independent runs",
    )
    accumulator.artifact_digests.add(digest)
    payload = value["payload"]
    require(isinstance(payload, dict), f"calibration run {position} raw payload is malformed")
    require_exact_keys(
        payload,
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
        f"calibration run {position} raw payload",
    )
    require(
        payload["schema_version"] == 1
        and all(payload[key] == expected for key, expected in expected_identity.items())
        and payload["clean"] is True
        and payload["unplanned_suspend"] is False,
        f"calibration run {position} raw payload identity is stale",
    )
    return payload


def _calibration_run(
    raw_run: object,
    *,
    position: int,
    bundle: CalibrationBundle,
    accumulator: CalibrationAccumulator,
) -> CalibrationRun:
    field = f"calibration run {position}"
    require(isinstance(raw_run, dict), f"{field} is malformed")
    require_exact_keys(
        raw_run,
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
        field,
    )
    run_id = require_sha256(raw_run["run_id_sha256"], f"{field}.run_id_sha256")
    require(
        run_id not in accumulator.run_ids,
        "calibration bundle duplicated an independent run id",
    )
    accumulator.run_ids.add(run_id)
    cell_id = require_nonempty_string(raw_run["matrix_cell_id"], f"{field}.matrix_cell_id")
    run_index = require_positive_int(raw_run["run_index"], f"{field}.run_index")
    run_cell = (cell_id, run_index)
    require(
        run_cell in accumulator.expected_run_cells
        and run_cell not in accumulator.observed_run_cells,
        f"calibration run {position} duplicated or escaped the exact matrix",
    )
    accumulator.observed_run_cells.add(run_cell)
    host = require_sha256(raw_run["host_fingerprint"], f"{field}.host_fingerprint")
    require(
        raw_run["clean"] is True and raw_run["unplanned_suspend"] is False,
        f"calibration run {position} was not a clean awake run",
    )
    require(
        raw_run["source"] == bundle.source
        and raw_run["contracts"] == bundle.contracts,
        f"calibration run {position} changed source, tree, or protocol identity",
    )
    matrix_cell = bundle.matrix[cell_id]
    package = _calibration_package(
        raw_run["package"],
        position=position,
        matrix_cell_id=cell_id,
        matrix_cell=matrix_cell,
        accumulator=accumulator,
    )
    payload = _calibration_raw_payload(
        raw_run["raw_artifact"],
        position=position,
        expected_identity={
            "run_id_sha256": run_id,
            "matrix_cell_id": cell_id,
            "run_index": run_index,
            "host_fingerprint": host,
            "source": bundle.source,
            "contracts": bundle.contracts,
            "package": package,
        },
        accumulator=accumulator,
    )
    metrics = payload["metrics"]
    require(
        isinstance(metrics, dict) and set(metrics) == set(accumulator.metric_values),
        f"calibration run {position} omitted a required metric",
    )
    return CalibrationRun(position, cell_id, matrix_cell, package, metrics)


_CALIBRATION_SAMPLE_FIELDS = {
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
}


def _calibration_sample(
    sample: object,
    *,
    metric: str,
    index: int,
    run: CalibrationRun,
    bundle: CalibrationBundle,
    accumulator: CalibrationAccumulator,
    maximum_suspend_ns: int,
) -> CalibrationSample:
    field = f"calibration run {run.position} metric {metric} sample {index}"
    require(isinstance(sample, dict), f"calibration {metric} sample is malformed")
    require_exact_keys(sample, _CALIBRATION_SAMPLE_FIELDS, field)
    sample_id = require_opaque_identifier(sample["sample_id"], f"{field} id")
    require(
        sample_id not in accumulator.sample_ids,
        "calibration bundle duplicated a sample identity",
    )
    accumulator.sample_ids.add(sample_id)
    require(
        sample["repeat"] == index + 1
        and sample["matrix_cell_id"] == run.matrix_cell_id
        and sample["workload_id"]
        == bundle.protocol["workloads"][metric]["workload_id"]
        and sample["cache_state"] == run.matrix_cell["cache_state"]
        and sample["residency_state"] == run.matrix_cell["residency_state"],
        f"calibration run {run.position} metric {metric} sample identity changed",
    )
    server = sample["server_identity"]
    require(isinstance(server, dict), f"calibration run {run.position} metric {metric} server identity is malformed")
    require_exact_keys(
        server,
        {"server_instance_id", "process_start_id", "load_generation"},
        f"calibration run {run.position} metric {metric} server identity",
    )
    identity = (
        require_opaque_identifier(server["server_instance_id"], f"{field} server_instance_id"),
        require_nonempty_string(server["process_start_id"], f"{field} process_start_id"),
        require_positive_int(server["load_generation"], f"{field} load_generation"),
    )
    target_os = TARGET_CONTRACTS[run.matrix_cell["asset_target"]]["target_os"]
    clock_policy = bundle.protocol["clock_policy"]
    value = qualification_measurement_sample_value(
        metric,
        sample,
        contracts=bundle.contracts,
        phase_boundaries=bundle.protocol["phase_boundaries"],
        allowed_awake_apis=set(clock_policy["platform_apis"][target_os]),
        inclusive_api=clock_policy["suspend_detection"]["platform_apis"][target_os],
        maximum_suspend_ns=maximum_suspend_ns,
        expected_policy=run.matrix_cell["policy"],
        expected_backend=run.matrix_cell["backend"],
    )
    return CalibrationSample(identity, value, sample)


def _record_calibration_durations(
    metric: str,
    sample: CalibrationSample,
    *,
    field: str,
    accumulator: CalibrationAccumulator,
) -> None:
    raw = sample.raw
    awake_ms = (raw["end"]["observed_ns"] - raw["start"]["observed_ns"]) / 1_000_000
    destinations = {
        "existing_owner_connect": "existing_owner_connect_duration",
        "spawn_convergence": "spawn_convergence_duration",
        "busy_retry_usefulness": "capacity_condition_duration",
    }
    if metric in destinations:
        accumulator.duration_values_ms[destinations[metric]].append(awake_ms)
    if metric in {"cold_first_vector", "first_product_ready", "warm_query_ipc"}:
        accumulator.duration_values_ms["query_request_duration"].append(awake_ms)
    bulk_metrics = {
        "warm_bulk_ipc",
        "bulk_documents_per_second",
        "bulk_tokens_per_second",
    }
    if metric in bulk_metrics:
        accumulator.duration_values_ms["bulk_request_duration"].append(awake_ms)
    successful = {
        "cold_first_vector",
        "first_product_ready",
        "warm_query_ipc",
        *bulk_metrics,
    }
    if metric in successful:
        duration = require_nonnegative_int(
            raw["operands"]["successful_operation_duration_ns"],
            f"{field} successful operation duration",
        )
        accumulator.duration_values_ms["successful_operation_duration"].append(
            duration / 1_000_000
        )


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


def _calibration_freeze(
    bundle: CalibrationBundle,
    accumulator: CalibrationAccumulator,
    selected_constants: dict,
    thresholds: dict,
    *,
    compare_frozen_constant_set: bool,
    frozen_source: dict | None,
    repository_root: Path | None,
    enforce_source_lineage: bool,
) -> tuple[str, dict | None]:
    if compare_frozen_constant_set:
        require(
            bundle.constant_set["calibration_required_values"] == selected_constants,
            "frozen compiled constants do not match the preregistered calibration formulas",
        )
        require(
            bundle.constant_set["qualification_thresholds"] == thresholds,
            "frozen qualification thresholds do not match the preregistered calibration formulas",
        )
    digests = sorted(accumulator.artifact_digests)
    freeze_digest = canonical_sha256(
        {
            "selection_protocol": bundle.raw["selection_protocol"],
            "source": bundle.source,
            "producer": bundle.producer,
            "contracts": bundle.contracts,
            "run_artifact_sha256s": digests,
            "calibration_required_values": selected_constants,
            "qualification_thresholds": thresholds,
        }
    )
    lineage = None
    if compare_frozen_constant_set and enforce_source_lineage:
        require(
            frozen_source is not None and repository_root is not None,
            "frozen qualification requires exact calibration-to-package source lineage",
        )
        lineage = verify_calibration_source_lineage(
            bundle.source,
            frozen_source,
            repository_root,
        )
    if compare_frozen_constant_set:
        _verify_calibration_freeze_record(
            bundle,
            digests,
            freeze_digest,
        )
    return freeze_digest, lineage


def _verify_calibration_freeze_record(
    bundle: CalibrationBundle,
    digests: list[str],
    freeze_digest: str,
) -> None:
    require(
        bundle.raw["freeze_digest"] == freeze_digest,
        "calibration bundle freeze digest does not match recomputed raw evidence",
    )
    record = bundle.constant_set["freeze_record"]
    require(
        record["selection_source_commit"] == bundle.source["commit"]
        and record["selection_source_tree"] == bundle.source["tree"]
        and record["measurement_protocol_sha256"]
        == bundle.contracts["measurement_protocol_sha256"]
        and record["protocol_sha256"] == bundle.contracts["protocol_sha256"]
        and record["input_constant_set_sha256"]
        == bundle.contracts["input_constant_set_sha256"]
        and record["calibration_bundle_sha256"] == sha256(bundle.path)
        and record["calibration_freeze_digest"] == freeze_digest
        and sorted(record["run_artifact_sha256s"]) == digests
        and record["selection_rule"]
        == "all_preregistered_clean_runs_no_outlier_removal",
        "constant-set freeze record does not bind the exact recomputed calibration bundle",
    )


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
    bundle = _calibration_bundle(
        path,
        measurement_contract,
        expected_producer_run_id=expected_producer_run_id,
        expected_producer_artifact=expected_producer_artifact,
    )
    accumulator = _verified_calibration_runs(bundle)
    selected_constants = _selected_calibration_constants(
        accumulator.duration_values_ms
    )
    thresholds = _selected_calibration_thresholds(
        accumulator.metric_values,
        bundle.protocol["metric_contracts"],
    )
    freeze_digest, source_lineage = _calibration_freeze(
        bundle,
        accumulator,
        selected_constants,
        thresholds,
        compare_frozen_constant_set=compare_frozen_constant_set,
        frozen_source=frozen_source,
        repository_root=repository_root,
        enforce_source_lineage=enforce_source_lineage,
    )
    return {
        "artifact": {"name": path.name, "sha256": sha256(path)},
        "source": bundle.source,
        "producer": bundle.producer,
        "contracts": bundle.contracts,
        "matrix_cell_count": len(bundle.matrix),
        "run_count": len(bundle.runs),
        "freeze_digest": freeze_digest,
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
        "run_artifact_sha256s": sorted(accumulator.artifact_digests),
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
    server_cleanup_control: dict,
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
    server_cleanup_control.update(
        {
            "qualification_cli": str(qualification_cli.resolve()),
            "qualification_directory": str(private_root.resolve()),
            "qualification_nonce": nonce,
            "plugin_cli_archive_sha256": archive_sha256,
            "projects": list(projects),
        }
    )
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
    server_cleanup_control["qualification_directory"] = str(
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
