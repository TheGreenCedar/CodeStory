"""Verification for retained calibration bundles."""

from __future__ import annotations

import json
import math
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    TARGET_CONTRACTS,
    ProofFailure,
    require,
)
from .contracts import (
    canonical_sha256,
    qualification_measurement_sample_value,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    sha256,
)
from .archive import (
    normalized_backend,
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

