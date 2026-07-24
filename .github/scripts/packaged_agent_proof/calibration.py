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
    require(path.is_file() and not path.is_symlink(), f"calibration bundle is missing or unsafe: {path}")
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
    source = bundle["source"]
    require(isinstance(source, dict), "calibration bundle source identity is malformed")
    require_exact_keys(source, {"commit", "tree", "tracked_dirty"}, "calibration bundle source")
    for field in ("commit", "tree"):
        require(
            isinstance(source[field], str)
            and re.fullmatch(r"[0-9a-f]{40}", source[field]) is not None,
            f"calibration bundle source.{field} is not an exact Git object id",
        )
    require(source["tracked_dirty"] is False, "calibration bundle source tree was dirty")
    producer = bundle["producer"]
    require(isinstance(producer, dict), "calibration bundle producer is malformed")
    require_exact_keys(
        producer,
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
        producer["repository"] == "TheGreenCedar/CodeStory"
        and producer["workflow_path"]
        == ".github/workflows/packaged-platform-pr.yml"
        and isinstance(producer["run_id"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
        and isinstance(producer["run_attempt"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"]) is not None
        and producer["artifact_name"]
        == f"embedding-calibration-bundle-{source['commit']}"
        and producer["source_head_sha"] == source["commit"],
        "calibration bundle producer is not the trusted exact-head coordinator artifact",
    )
    if expected_producer_run_id is not None or expected_producer_artifact is not None:
        require(
            producer["run_id"] == expected_producer_run_id
            and producer["artifact_name"] == expected_producer_artifact,
            "calibration bundle producer differs from the authenticated download request",
        )
    contracts = bundle["contracts"]
    expected_contracts = {
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
        contracts == expected_contracts,
        "calibration bundle contract hashes differ from the checked-in protocols",
    )
    runs = bundle["runs"]
    matrix = protocol["calibration_matrix"]
    expected_run_cells = {
        (matrix_cell_id, run_index)
        for matrix_cell_id in matrix
        for run_index in range(1, 4)
    }
    require(
        isinstance(runs, list) and len(runs) == len(expected_run_cells),
        "calibration bundle must contain exactly three runs for every matrix cell",
    )
    observed_run_cells: set[tuple[str, int]] = set()
    run_ids: set[str] = set()
    artifact_digests: set[str] = set()
    packages_by_cell: dict[str, dict] = {}
    global_sample_ids: set[str] = set()
    calibration_metrics = set(protocol["required_metrics"]) - {"retrieval_quality"}
    run_metric_values: dict[str, list[float | int]] = {
        metric: [] for metric in calibration_metrics
    }
    raw_duration_values_ms: dict[str, list[float]] = {
        "existing_owner_connect_duration": [],
        "spawn_convergence_duration": [],
        "query_request_duration": [],
        "bulk_request_duration": [],
        "capacity_condition_duration": [],
        "successful_operation_duration": [],
    }
    phase_boundaries = protocol["phase_boundaries"]
    metric_contracts = protocol["metric_contracts"]
    sampling = protocol["metric_sampling"]
    suspend_contract = protocol["clock_policy"]["suspend_detection"]
    maximum_suspend_ns = require_nonnegative_int(
        suspend_contract["maximum_inclusive_minus_awake_ns"],
        "calibration suspend-detection tolerance",
    )
    for run_position, run in enumerate(runs):
        require(isinstance(run, dict), f"calibration run {run_position} is malformed")
        require_exact_keys(
            run,
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
            f"calibration run {run_position}",
        )
        run_id = require_sha256(
            run["run_id_sha256"],
            f"calibration run {run_position}.run_id_sha256",
        )
        require(run_id not in run_ids, "calibration bundle duplicated an independent run id")
        run_ids.add(run_id)
        matrix_cell_id = require_nonempty_string(
            run["matrix_cell_id"],
            f"calibration run {run_position}.matrix_cell_id",
        )
        run_index = require_positive_int(
            run["run_index"],
            f"calibration run {run_position}.run_index",
        )
        run_cell = (matrix_cell_id, run_index)
        require(
            run_cell in expected_run_cells and run_cell not in observed_run_cells,
            f"calibration run {run_position} duplicated or escaped the exact matrix",
        )
        observed_run_cells.add(run_cell)
        host_fingerprint = require_sha256(
            run["host_fingerprint"],
            f"calibration run {run_position}.host_fingerprint",
        )
        require(
            run["clean"] is True and run["unplanned_suspend"] is False,
            f"calibration run {run_position} was not a clean awake run",
        )
        require(
            run["source"] == source and run["contracts"] == contracts,
            f"calibration run {run_position} changed source, tree, or protocol identity",
        )
        matrix_cell = matrix[matrix_cell_id]
        package = run["package"]
        require(isinstance(package, dict), f"calibration run {run_position} package is malformed")
        require_exact_keys(
            package,
            {
                "archive_sha256",
                "executable_sha256",
                "asset_target",
                "release_version",
                "model_sha256",
                "policy",
                "backend",
            },
            f"calibration run {run_position} package",
        )
        for field in ("archive_sha256", "executable_sha256", "model_sha256"):
            require_sha256(package[field], f"calibration run {run_position} package.{field}")
        require(
            package["asset_target"] == matrix_cell["asset_target"]
            and package["policy"] == matrix_cell["policy"]
            and normalized_backend(package["backend"])
            == normalized_backend(matrix_cell["backend"])
            and isinstance(package["release_version"], str)
            and bool(package["release_version"]),
            f"calibration run {run_position} package does not match its matrix cell",
        )
        if matrix_cell_id in packages_by_cell:
            require(
                package == packages_by_cell[matrix_cell_id],
                f"calibration matrix cell {matrix_cell_id} changed package between runs",
            )
        else:
            packages_by_cell[matrix_cell_id] = package
        raw_artifact = run["raw_artifact"]
        require(isinstance(raw_artifact, dict), f"calibration run {run_position} raw artifact is malformed")
        require_exact_keys(
            raw_artifact,
            {"name", "sha256", "payload"},
            f"calibration run {run_position} raw artifact",
        )
        require(
            raw_artifact["name"] == "measurements.raw.json",
            f"calibration run {run_position} raw artifact has the wrong name",
        )
        artifact_digest = require_sha256(
            raw_artifact["sha256"],
            f"calibration run {run_position} raw artifact sha256",
        )
        require(
            artifact_digest == canonical_sha256(raw_artifact["payload"]),
            f"calibration run {run_position} raw artifact digest does not match its payload",
        )
        require(
            artifact_digest not in artifact_digests,
            "calibration bundle reused one raw artifact for multiple independent runs",
        )
        artifact_digests.add(artifact_digest)
        raw = raw_artifact["payload"]
        require(isinstance(raw, dict), f"calibration run {run_position} raw payload is malformed")
        require_exact_keys(
            raw,
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
            f"calibration run {run_position} raw payload",
        )
        require(
            raw["schema_version"] == 1
            and raw["run_id_sha256"] == run_id
            and raw["matrix_cell_id"] == matrix_cell_id
            and raw["run_index"] == run_index
            and raw["host_fingerprint"] == host_fingerprint
            and raw["source"] == source
            and raw["contracts"] == contracts
            and raw["package"] == package
            and raw["clean"] is True
            and raw["unplanned_suspend"] is False,
            f"calibration run {run_position} raw payload identity is stale",
        )
        metrics = raw["metrics"]
        require(
            isinstance(metrics, dict)
            and set(metrics) == calibration_metrics,
            f"calibration run {run_position} omitted a required metric",
        )
        target_os = TARGET_CONTRACTS[matrix_cell["asset_target"]]["target_os"]
        allowed_awake_apis = set(protocol["clock_policy"]["platform_apis"][target_os])
        inclusive_api = suspend_contract["platform_apis"][target_os]
        for metric in sorted(metrics):
            record = metrics[metric]
            require(isinstance(record, dict), f"calibration run {run_position} metric {metric} is malformed")
            require_exact_keys(
                record,
                {"unit", "samples"},
                f"calibration run {run_position} metric {metric}",
            )
            require(
                record["unit"] == metric_contracts[metric]["unit"],
                f"calibration run {run_position} metric {metric} used the wrong unit",
            )
            samples = record["samples"]
            require(
                isinstance(samples, list)
                and len(samples) == sampling[metric]["sample_count"],
                f"calibration run {run_position} metric {metric} sample count changed",
            )
            values = []
            server_identities = []
            for sample_index, sample in enumerate(samples):
                require(isinstance(sample, dict), f"calibration {metric} sample is malformed")
                require_exact_keys(
                    sample,
                    {
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
                    },
                    f"calibration run {run_position} metric {metric} sample {sample_index}",
                )
                sample_id = require_opaque_identifier(
                    sample["sample_id"],
                    f"calibration run {run_position} metric {metric} sample id",
                )
                require(
                    sample_id not in global_sample_ids,
                    "calibration bundle duplicated a sample identity",
                )
                global_sample_ids.add(sample_id)
                require(
                    sample["repeat"] == sample_index + 1
                    and sample["matrix_cell_id"] == matrix_cell_id
                    and sample["workload_id"] == protocol["workloads"][metric]["workload_id"]
                    and sample["cache_state"] == matrix_cell["cache_state"]
                    and sample["residency_state"] == matrix_cell["residency_state"],
                    f"calibration run {run_position} metric {metric} sample identity changed",
                )
                server_identity = sample["server_identity"]
                require(
                    isinstance(server_identity, dict),
                    f"calibration run {run_position} metric {metric} server identity is malformed",
                )
                require_exact_keys(
                    server_identity,
                    {"server_instance_id", "process_start_id", "load_generation"},
                    f"calibration run {run_position} metric {metric} server identity",
                )
                server_identities.append(
                    (
                        require_opaque_identifier(
                            server_identity["server_instance_id"],
                            f"calibration run {run_position} metric {metric} server_instance_id",
                        ),
                        require_nonempty_string(
                            server_identity["process_start_id"],
                            f"calibration run {run_position} metric {metric} process_start_id",
                        ),
                        require_positive_int(
                            server_identity["load_generation"],
                            f"calibration run {run_position} metric {metric} load_generation",
                        ),
                    )
                )
                value = qualification_measurement_sample_value(
                    metric,
                    sample,
                    contracts=contracts,
                    phase_boundaries=phase_boundaries,
                    allowed_awake_apis=allowed_awake_apis,
                    inclusive_api=inclusive_api,
                    maximum_suspend_ns=maximum_suspend_ns,
                    expected_policy=matrix_cell["policy"],
                    expected_backend=matrix_cell["backend"],
                )
                values.append(value)
                awake_delta_ms = (
                    sample["end"]["observed_ns"] - sample["start"]["observed_ns"]
                ) / 1_000_000
                if metric == "existing_owner_connect":
                    raw_duration_values_ms["existing_owner_connect_duration"].append(
                        awake_delta_ms
                    )
                if metric == "spawn_convergence":
                    raw_duration_values_ms["spawn_convergence_duration"].append(
                        awake_delta_ms
                    )
                if metric in {"cold_first_vector", "first_product_ready", "warm_query_ipc"}:
                    raw_duration_values_ms["query_request_duration"].append(awake_delta_ms)
                if metric in {
                    "warm_bulk_ipc",
                    "bulk_documents_per_second",
                    "bulk_tokens_per_second",
                }:
                    raw_duration_values_ms["bulk_request_duration"].append(awake_delta_ms)
                if metric == "busy_retry_usefulness":
                    raw_duration_values_ms["capacity_condition_duration"].append(
                        awake_delta_ms
                    )
                if metric in {
                    "cold_first_vector",
                    "first_product_ready",
                    "warm_query_ipc",
                    "warm_bulk_ipc",
                    "bulk_documents_per_second",
                    "bulk_tokens_per_second",
                }:
                    raw_duration_values_ms["successful_operation_duration"].append(
                        require_nonnegative_int(
                            sample["operands"]["successful_operation_duration_ns"],
                            f"calibration run {run_position} metric {metric} successful operation duration",
                        )
                        / 1_000_000
                    )
            if sampling[metric].get("independence") == "distinct_server_instance_per_sample":
                require(
                    len({identity[:2] for identity in server_identities}) == len(samples),
                    f"calibration run {run_position} metric {metric} samples are not independent",
                )
            else:
                require(
                    len(set(server_identities)) == 1,
                    f"calibration run {run_position} metric {metric} changed server identity",
                )
            aggregation = sampling[metric]["aggregation"]
            run_metric_values[metric].append(
                {
                    "maximum": max,
                    "minimum": min,
                    "exact": lambda raw_values: raw_values[0],
                    "all_rows_pass_rate": lambda raw_values: sum(raw_values)
                    / len(raw_values),
                }[aggregation](values)
            )
    require(
        observed_run_cells == expected_run_cells,
        "calibration bundle does not exactly cover every matrix cell three times",
    )
    require(
        len({package["release_version"] for package in packages_by_cell.values()}) == 1
        and len({package["model_sha256"] for package in packages_by_cell.values()}) == 1,
        "calibration matrix cells did not use one release version and model",
    )
    require(
        all(raw_duration_values_ms.values()),
        "calibration bundle omitted a production-constant raw source cell",
    )
    connect_timeout_ms = max(
        1, math.ceil(max(raw_duration_values_ms["existing_owner_connect_duration"]) * 1.50)
    )
    spawn_timeout_ms = max(
        1, math.ceil(max(raw_duration_values_ms["spawn_convergence_duration"]) * 1.50)
    )
    query_deadline_ms = max(
        1, math.ceil(max(raw_duration_values_ms["query_request_duration"]) * 1.50)
    )
    bulk_replay_success_budget_ms = max(
        query_deadline_ms,
        math.ceil(max(raw_duration_values_ms["bulk_request_duration"]) * 1.50),
    )
    retry_after_ms = max(
        1, math.floor(min(raw_duration_values_ms["capacity_condition_duration"]) * 0.50)
    )
    initial_backoff_ms = max(
        1,
        math.ceil(
            max(raw_duration_values_ms["existing_owner_connect_duration"]) * 0.50
        ),
    )
    maximum_backoff_ms = max(
        initial_backoff_ms,
        math.ceil(max(raw_duration_values_ms["spawn_convergence_duration"]) * 0.25),
    )
    hard_no_progress_ms = max(
        1,
        math.ceil(max(raw_duration_values_ms["successful_operation_duration"]) * 4.00),
    )
    watchdog_cadence_ms = max(1, math.floor(hard_no_progress_ms / 20))
    bulk_deadline_ms = (
        hard_no_progress_ms
        + watchdog_cadence_ms
        + spawn_timeout_ms
        + bulk_replay_success_budget_ms
    )
    selected_constants = {
        "connect_timeout_ms": connect_timeout_ms,
        "spawn_convergence_timeout_ms": spawn_timeout_ms,
        "request_deadlines_ms": {
            "query_request_deadline_ms": query_deadline_ms,
            "bulk_replay_success_budget_ms": bulk_replay_success_budget_ms,
            "bulk_request_deadline_ms": bulk_deadline_ms,
        },
        "capacity_retry_policy": {
            "retry_after_ms": retry_after_ms,
            "retry_class": "after_capacity_change",
            "retry_condition_source": "named_condition_from_typed_capacity_response",
        },
        "election_backoff_policy": {
            "initial_backoff_ms": initial_backoff_ms,
            "maximum_backoff_ms": maximum_backoff_ms,
            "jitter": (
                "sha256(process_start_id||attempt) modulo inclusive "
                "[initial_backoff_ms,maximum_backoff_ms]"
            ),
        },
        "hard_native_no_progress_ms": hard_no_progress_ms,
        "watchdog_cadence_ms": watchdog_cadence_ms,
    }
    thresholds: dict[str, float | int] = {}
    for metric, values in run_metric_values.items():
        comparison = metric_contracts[metric]["comparison"]
        if metric == "retrieval_quality":
            threshold: float | int = 1.0
        elif comparison == "less_than_or_equal":
            threshold = math.ceil(max(values) * 1.20)
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
    if compare_frozen_constant_set:
        require(
            constant_set["calibration_required_values"] == selected_constants,
            "frozen compiled constants do not match the preregistered calibration formulas",
        )
        require(
            constant_set["qualification_thresholds"] == thresholds,
            "frozen qualification thresholds do not match the preregistered calibration formulas",
        )
    freeze_payload = {
        "selection_protocol": bundle["selection_protocol"],
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "run_artifact_sha256s": sorted(artifact_digests),
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
    }
    freeze_digest = canonical_sha256(freeze_payload)
    source_lineage = None
    if compare_frozen_constant_set and enforce_source_lineage:
        require(
            frozen_source is not None and repository_root is not None,
            "frozen qualification requires exact calibration-to-package source lineage",
        )
        source_lineage = verify_calibration_source_lineage(
            source,
            frozen_source,
            repository_root,
        )
    if compare_frozen_constant_set:
        require(
            bundle["freeze_digest"] == freeze_digest,
            "calibration bundle freeze digest does not match recomputed raw evidence",
        )
        freeze_record = constant_set["freeze_record"]
        require(
            freeze_record["selection_source_commit"] == source["commit"]
            and freeze_record["selection_source_tree"] == source["tree"]
            and freeze_record["measurement_protocol_sha256"]
            == contracts["measurement_protocol_sha256"]
            and freeze_record["protocol_sha256"] == contracts["protocol_sha256"]
            and freeze_record["input_constant_set_sha256"]
            == contracts["input_constant_set_sha256"]
            and freeze_record["calibration_bundle_sha256"] == sha256(path)
            and freeze_record["calibration_freeze_digest"] == freeze_digest
            and sorted(freeze_record["run_artifact_sha256s"])
            == sorted(artifact_digests)
            and freeze_record["selection_rule"]
            == "all_preregistered_clean_runs_no_outlier_removal",
            "constant-set freeze record does not bind the exact recomputed calibration bundle",
        )
    return {
        "artifact": {"name": path.name, "sha256": sha256(path)},
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "matrix_cell_count": len(matrix),
        "run_count": len(runs),
        "freeze_digest": freeze_digest,
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
        "run_artifact_sha256s": sorted(artifact_digests),
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
