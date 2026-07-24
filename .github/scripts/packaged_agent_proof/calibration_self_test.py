"""Synthetic calibration bundle construction for packaged-proof self-tests."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path

from .contract_primitives import canonical_sha256, sha256, write_json
from .foundation import TARGET_CONTRACTS

_MEMORY_ROLES = (
    "plugin_host_a",
    "plugin_cli_a",
    "plugin_host_b",
    "plugin_cli_b",
    "embedding_server",
)

_SUCCESSFUL_METRICS = {
    "cold_first_vector",
    "first_product_ready",
    "warm_query_ipc",
    "warm_bulk_ipc",
    "bulk_documents_per_second",
    "bulk_tokens_per_second",
}


@dataclass(frozen=True)
class SelfTestRunContext:
    protocol: dict
    contracts: dict
    source: dict
    matrix_cell_id: str
    cell: dict
    cell_position: int
    run_index: int
    awake_api: str
    inclusive_api: str

    @property
    def seed(self) -> str:
        return f"{self.matrix_cell_id}:{self.run_index}"


def _self_test_operands(
    metric: str,
    *,
    pid: int,
    duration_ns: int,
    cell: dict,
) -> dict:
    operands: dict[str, object] = {}
    if metric in _SUCCESSFUL_METRICS:
        operands["successful_operation_duration_ns"] = duration_ns
    if metric == "bulk_documents_per_second":
        operands["completed_documents"] = 1
    elif metric == "bulk_tokens_per_second":
        operands["completed_tokens"] = 1
    elif metric == "total_codestory_process_memory":
        operands["processes"] = [
            {
                "role": role,
                "pid": pid + index + 1,
                "process_start_id": f"boot:{pid + index + 1}",
                "executable_sha256": hashlib.sha256(f"exe:{role}".encode()).hexdigest(),
                "resident_bytes": 1,
                "measurement_api": "self_test",
            }
            for index, role in enumerate(_MEMORY_ROLES)
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
    return operands


def _self_test_sample(
    context: SelfTestRunContext,
    metric: str,
    metric_position: int,
    repeat: int,
) -> dict:
    policy = context.protocol["metric_sampling"][metric]
    pid = (
        10_000
        + context.cell_position * 1_000
        + context.run_index * 100
        + metric_position * 10
        + repeat
    )
    independent = policy.get("independence") == "distinct_server_instance_per_sample"
    identity_seed = (
        f"{context.seed}:{metric}:{repeat}"
        if independent
        else f"{context.seed}:{metric}"
    )
    server_id = "server:" + hashlib.sha256(identity_seed.encode()).hexdigest()
    server_start = (
        f"boot:{pid}"
        if independent
        else "boot:"
        + hashlib.sha256(f"server-start:{context.seed}:{metric}".encode()).hexdigest()
    )
    started_ns = repeat * 2_000_000
    finished_ns = started_ns + 1_000_000
    boot_id = f"boot-{context.cell_position}"
    return {
        "sample_id": "sample:"
        + hashlib.sha256(f"{context.seed}:{metric}:{repeat}".encode()).hexdigest(),
        "repeat": repeat,
        "matrix_cell_id": context.matrix_cell_id,
        "workload_id": context.protocol["workloads"][metric]["workload_id"],
        "cache_state": context.cell["cache_state"],
        "residency_state": context.cell["residency_state"],
        "process": {"pid": pid, "process_start_id": f"boot:{pid}"},
        "server_identity": {
            "server_instance_id": server_id,
            "process_start_id": server_start,
            "load_generation": 1,
        },
        "clock": {
            "domain": "awake_monotonic",
            "api": context.awake_api,
            "boot_id": boot_id,
            "resolution_ns": 1,
        },
        "start": {
            "phase": context.protocol["phase_boundaries"][metric][0],
            "observed_ns": started_ns,
        },
        "end": {
            "phase": context.protocol["phase_boundaries"][metric][1],
            "observed_ns": finished_ns,
        },
        "operands": _self_test_operands(
            metric,
            pid=pid,
            duration_ns=finished_ns - started_ns,
            cell=context.cell,
        ),
        "suspend_witness": {
            "awake_started_ns": started_ns,
            "awake_finished_ns": finished_ns,
            "inclusive_clock_api": context.inclusive_api,
            "inclusive_started_ns": started_ns,
            "inclusive_finished_ns": finished_ns,
            "boot_id_started": boot_id,
            "boot_id_finished": boot_id,
        },
    }


def _self_test_metrics(context: SelfTestRunContext) -> dict:
    metrics = {}
    names = sorted(set(context.protocol["required_metrics"]) - {"retrieval_quality"})
    for position, metric in enumerate(names):
        policy = context.protocol["metric_sampling"][metric]
        metrics[metric] = {
            "unit": context.protocol["metric_contracts"][metric]["unit"],
            "samples": [
                _self_test_sample(context, metric, position, repeat)
                for repeat in range(1, policy["sample_count"] + 1)
            ],
        }
    return metrics


def _self_test_run(context: SelfTestRunContext) -> tuple[dict, str]:
    run_id = hashlib.sha256(f"run:{context.seed}".encode()).hexdigest()
    host = hashlib.sha256(f"host:{context.matrix_cell_id}".encode()).hexdigest()
    package = {
        "archive_sha256": hashlib.sha256(
            f"archive:{context.matrix_cell_id}".encode()
        ).hexdigest(),
        "executable_sha256": hashlib.sha256(
            f"executable:{context.matrix_cell_id}".encode()
        ).hexdigest(),
        "asset_target": context.cell["asset_target"],
        "release_version": "0.0.0",
        "model_sha256": hashlib.sha256(b"model").hexdigest(),
        "policy": context.cell["policy"],
        "backend": context.cell["backend"],
    }
    payload = {
        "schema_version": 1,
        "run_id_sha256": run_id,
        "matrix_cell_id": context.matrix_cell_id,
        "run_index": context.run_index,
        "host_fingerprint": host,
        "source": context.source,
        "contracts": context.contracts,
        "package": package,
        "clean": True,
        "unplanned_suspend": False,
        "metrics": _self_test_metrics(context),
    }
    digest = canonical_sha256(payload)
    return (
        {
            "run_id_sha256": run_id,
            "matrix_cell_id": context.matrix_cell_id,
            "run_index": context.run_index,
            "host_fingerprint": host,
            "clean": True,
            "unplanned_suspend": False,
            "source": context.source,
            "contracts": context.contracts,
            "package": package,
            "raw_artifact": {
                "name": "measurements.raw.json",
                "sha256": digest,
                "payload": payload,
            },
        },
        digest,
    )


def _self_test_runs(
    protocol: dict,
    contracts: dict,
    source: dict,
) -> tuple[list[dict], list[str]]:
    runs = []
    digests = []
    for position, (cell_id, cell) in enumerate(
        sorted(protocol["calibration_matrix"].items())
    ):
        target_os = TARGET_CONTRACTS[cell["asset_target"]]["target_os"]
        for run_index in range(1, 4):
            context = SelfTestRunContext(
                protocol,
                contracts,
                source,
                cell_id,
                cell,
                position,
                run_index,
                protocol["clock_policy"]["platform_apis"][target_os][0],
                protocol["clock_policy"]["suspend_detection"]["platform_apis"][
                    target_os
                ],
            )
            run, digest = _self_test_run(context)
            runs.append(run)
            digests.append(digest)
    return runs, digests


def _self_test_selection(protocol: dict) -> tuple[dict, dict]:
    constants = {
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
            1.0
            if metric == "retrieval_quality"
            else 1
            if metric == "backend_observed_accelerator_residency"
            else 800
            if metric in {"bulk_documents_per_second", "bulk_tokens_per_second"}
            else 6
            if metric == "total_codestory_process_memory"
            else 2
        )
        for metric in protocol["required_metrics"]
    }
    return constants, thresholds


def _frozen_self_test_contract(
    measurement_contract: dict,
    *,
    source: dict,
    contracts: dict,
    path: Path,
    bundle: dict,
    digests: list[str],
    constants: dict,
    thresholds: dict,
) -> dict:
    frozen = json.loads(json.dumps(measurement_contract))
    constant_set = frozen["constant_set"]
    constant_set["status"] = "frozen"
    constant_set["calibration_required_values"] = constants
    constant_set["qualification_thresholds"] = thresholds
    constant_set["freeze_record"] = {
        "selection_source_commit": source["commit"],
        "selection_source_tree": source["tree"],
        "measurement_protocol_sha256": contracts["measurement_protocol_sha256"],
        "protocol_sha256": contracts["protocol_sha256"],
        "input_constant_set_sha256": contracts["input_constant_set_sha256"],
        "calibration_bundle_sha256": sha256(path),
        "calibration_freeze_digest": bundle["freeze_digest"],
        "run_artifact_sha256s": sorted(digests),
        "selection_rule": "all_preregistered_clean_runs_no_outlier_removal",
        "selected_at": "self-test",
    }
    return frozen


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
    runs, digests = _self_test_runs(protocol, contracts, source)
    constants, thresholds = _self_test_selection(protocol)
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
        "run_artifact_sha256s": sorted(digests),
        "calibration_required_values": constants,
        "qualification_thresholds": thresholds,
    }
    bundle = {
        "schema_version": 1,
        "selection_protocol": freeze_payload["selection_protocol"],
        "source": source,
        "producer": producer,
        "contracts": contracts,
        "runs": runs,
        "freeze_digest": canonical_sha256(freeze_payload),
    }
    path = root / "calibration-bundle.json"
    write_json(path, bundle)
    frozen = _frozen_self_test_contract(
        measurement_contract,
        source=source,
        contracts=contracts,
        path=path,
        bundle=bundle,
        digests=digests,
        constants=constants,
        thresholds=thresholds,
    )
    return path, frozen, bundle
