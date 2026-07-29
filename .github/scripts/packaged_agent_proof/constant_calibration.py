"""Constant-only calibration collection from one authenticated package."""

from __future__ import annotations

import hashlib
import json
import platform
import secrets
import time
from pathlib import Path

from .contract_primitives import (
    canonical_sha256,
    normalized_backend,
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
    require_sha256,
    sha256,
    write_json,
    write_private_json,
)
from .failure_evidence import register_failure_evidence_secret
from .foundation import NATIVE_MANIFEST_FILE, ProofFailure, TARGET_CONTRACTS, require
from .measurement_samples import qualification_measurement_sample_value
from .native_manifest import runtime_executable_path, runtime_executable_sha256
from .subprocess_control import run

_RUN_COUNT = 3
_METRIC_COUNT = 9
_SAMPLE_FIELDS = {
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


def _constant_calibration_matrix_cell(args, protocol: dict, manifest: dict) -> dict:
    cell_id = require_nonempty_string(
        args.qualification_matrix_cell,
        "--collect-constant-calibration requires --qualification-matrix-cell",
    )
    required = protocol["calibration_matrix"]
    optional = protocol["optional_calibration_evidence_matrix"]
    require(
        not (cell_id in required and cell_id in optional),
        f"constant-calibration matrix cell {cell_id} is duplicated",
    )
    cell = required.get(cell_id) or optional.get(cell_id)
    require(cell is not None, f"unknown constant-calibration matrix cell {cell_id!r}")
    require(
        cell["asset_target"] == manifest["asset_target"]
        and cell["proof_tier"] == "calibration"
        and cell["policy"] == "accelerated"
        and normalized_backend(cell["backend"]) in {"metal", "vulkan"}
        and cell["cache_state"] == "reused"
        and cell["residency_state"] == "resident",
        "constant-calibration matrix cell is not an accelerated GPU lane",
    )
    require(
        args.engine_policy == "accelerated"
        and args.offline
        and normalized_backend(args.expected_backend)
        == normalized_backend(cell["backend"]),
        "constant calibration requires offline accelerated execution on its declared GPU backend",
    )
    return cell


def _prepare_synthetic_project(private_root: Path) -> Path:
    project = private_root / "synthetic-project"
    project.mkdir(mode=0o700)
    (project / "README.md").write_text(
        "# Constant calibration fixture\n\nOne project prepared once per package cell.\n",
        encoding="utf-8",
    )
    (project / "lib.rs").write_text(
        'pub fn constant_calibration_probe() -> &\'static str { "gpu" }\n',
        encoding="utf-8",
    )
    return project.resolve()


def _native_manifest_path(unpacked_root: Path) -> Path:
    matches = [
        path
        for path in unpacked_root.rglob(NATIVE_MANIFEST_FILE)
        if path.is_file() and not path.is_symlink()
    ]
    require(
        len(matches) == 1,
        "constant calibration requires exactly one authenticated native manifest",
    )
    return matches[0].resolve()


def _load_json(path: Path, field: str) -> dict:
    require(
        path.is_file() and not path.is_symlink(),
        f"{field} is missing or unsafe: {path}",
    )
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{field} is not valid JSON: {exc}") from exc
    require(isinstance(value, dict), f"{field} must be an object")
    return value


def _validate_driver_output(
    output: dict,
    *,
    request: dict,
    request_path: Path,
    private_root: Path,
    protocol: dict,
    matrix_cell: dict,
) -> list[tuple[dict, dict, str]]:
    require_exact_keys(
        output,
        {
            "schema_version",
            "source",
            "package",
            "contracts",
            "runtime",
            "request_sha256",
            "calibration_runs",
        },
        "constant-calibration driver output",
    )
    require(
        output["schema_version"] == 1
        and output["source"] == request["source"]
        and output["package"] == request["package"]
        and output["contracts"] == request["contracts"]
        and output["runtime"] == request["runtime"]
        and output["request_sha256"] == sha256(request_path),
        "constant-calibration driver output changed its authenticated request identity",
    )
    summaries = output["calibration_runs"]
    require(
        isinstance(summaries, list) and len(summaries) == _RUN_COUNT,
        "constant-calibration driver must return exactly three clean runs",
    )
    target_os = TARGET_CONTRACTS[matrix_cell["asset_target"]]["target_os"]
    allowed_awake_apis = set(protocol["clock_policy"]["platform_apis"][target_os])
    inclusive_api = protocol["clock_policy"]["suspend_detection"]["platform_apis"][
        target_os
    ]
    maximum_suspend_ns = protocol["clock_policy"]["suspend_detection"][
        "maximum_inclusive_minus_awake_ns"
    ]
    expected_metrics = set(protocol["calibration_required_metrics"])
    retained = []
    observed_generations: set[tuple[str, str, int]] = set()
    for expected_index, summary in enumerate(summaries, start=1):
        field = f"constant-calibration run {expected_index}"
        require(isinstance(summary, dict), f"{field} summary is malformed")
        require_exact_keys(
            summary,
            {
                "run_index",
                "measurements",
                "server_identities",
                "backend",
                "policy",
                "model_sha256",
                "materialized_reused",
            },
            f"{field} summary",
        )
        require(
            summary["run_index"] == expected_index
            and summary["policy"] == "accelerated"
            and normalized_backend(summary["backend"])
            == normalized_backend(matrix_cell["backend"])
            and summary["model_sha256"] == request["package"]["model_sha256"]
            and summary["materialized_reused"] is (expected_index > 1),
            f"{field} changed backend, model, or materialization identity",
        )
        measurements = summary["measurements"]
        require(
            isinstance(measurements, dict)
            and measurements
            == {
                "artifact": f"constant-calibration-run-{expected_index}.raw.json",
                "metric_count": _METRIC_COUNT,
                "sample_count": _METRIC_COUNT,
            },
            f"{field} did not retain one sample for each constant-source metric",
        )
        artifact_path = private_root / measurements["artifact"]
        raw = _load_json(artifact_path, f"{field} raw artifact")
        require_exact_keys(
            raw,
            {
                "schema_version",
                "run_index",
                "contracts",
                "metrics",
                "server_identities",
                "backend",
                "policy",
                "model_sha256",
                "materialized_reused",
            },
            f"{field} raw artifact",
        )
        require(
            raw["schema_version"] == 1
            and raw["run_index"] == expected_index
            and raw["contracts"] == request["contracts"]
            and raw["backend"] == summary["backend"]
            and raw["policy"] == summary["policy"]
            and raw["model_sha256"] == summary["model_sha256"]
            and raw["materialized_reused"] == summary["materialized_reused"]
            and raw["server_identities"] == summary["server_identities"],
            f"{field} summary does not bind its raw artifact",
        )
        identities = raw["server_identities"]
        require(
            isinstance(identities, list) and len(identities) == 1,
            f"{field} must use one fresh server generation",
        )
        identity = identities[0]
        require(isinstance(identity, dict), f"{field} server identity is malformed")
        require_exact_keys(
            identity,
            {"server_instance_id", "process_start_id", "load_generation"},
            f"{field} server identity",
        )
        generation = (
            require_nonempty_string(
                identity["server_instance_id"],
                f"{field} server_instance_id",
            ),
            require_nonempty_string(
                identity["process_start_id"],
                f"{field} process_start_id",
            ),
            require_positive_int(
                identity["load_generation"],
                f"{field} load_generation",
            ),
        )
        require(
            generation not in observed_generations,
            "constant calibration reused a server generation across clean runs",
        )
        observed_generations.add(generation)
        metrics = raw["metrics"]
        require(
            isinstance(metrics, dict) and set(metrics) == expected_metrics,
            f"{field} included qualification-only or omitted constant-source metrics",
        )
        for metric, record in metrics.items():
            require(isinstance(record, dict), f"{field} metric {metric} is malformed")
            require_exact_keys(record, {"unit", "samples"}, f"{field} metric {metric}")
            require(
                record["unit"] == protocol["metric_contracts"][metric]["unit"]
                and isinstance(record["samples"], list)
                and len(record["samples"]) == 1,
                f"{field} metric {metric} must retain exactly one declared sample",
            )
            sample = record["samples"][0]
            require(
                isinstance(sample, dict),
                f"{field} metric {metric} sample is malformed",
            )
            require_exact_keys(
                sample,
                _SAMPLE_FIELDS,
                f"{field} metric {metric} sample",
            )
            require(
                sample["repeat"] == 1
                and sample["matrix_cell_id"] == request["runtime"]["matrix_cell_id"]
                and sample["workload_id"] == protocol["workloads"][metric]["workload_id"]
                and sample["cache_state"] == matrix_cell["cache_state"]
                and sample["residency_state"] == matrix_cell["residency_state"]
                and sample["server_identity"] == identity,
                f"{field} metric {metric} escaped its declared workload or server generation",
            )
            qualification_measurement_sample_value(
                metric,
                sample,
                contracts=request["contracts"],
                phase_boundaries=protocol["calibration_phase_boundaries"],
                allowed_awake_apis=allowed_awake_apis,
                inclusive_api=inclusive_api,
                maximum_suspend_ns=maximum_suspend_ns,
                expected_policy="accelerated",
                expected_backend=matrix_cell["backend"],
            )
        retained.append((summary, raw, sha256(artifact_path)))
    return retained


def _host_fingerprint() -> str:
    identity = "|".join(
        (
            platform.system(),
            platform.machine(),
            platform.release(),
            platform.node(),
        )
    )
    return hashlib.sha256(identity.encode("utf-8")).hexdigest()


def _retained_run(
    *,
    summary: dict,
    raw: dict,
    physical_artifact_sha256: str,
    manifest: dict,
    measurement_contract: dict,
    matrix_cell_id: str,
    matrix_cell: dict,
    archive_sha256: str,
    host_fingerprint: str,
    request_sha256: str,
) -> dict:
    run_index = summary["run_index"]
    contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "measurement_protocol_sha256": measurement_contract[
            "measurement_protocol_sha256"
        ],
        "input_constant_set_sha256": measurement_contract["constant_set_sha256"],
    }
    package = {
        "archive_sha256": require_sha256(
            archive_sha256,
            "constant-calibration archive sha256",
        ),
        "executable_sha256": runtime_executable_sha256(manifest),
        "asset_target": manifest["asset_target"],
        "release_version": manifest["release_version"],
        "model_sha256": summary["model_sha256"],
        "policy": "accelerated",
        "backend": normalized_backend(matrix_cell["backend"]),
    }
    run_id = canonical_sha256(
        {
            "source": manifest["source"],
            "package": package,
            "matrix_cell_id": matrix_cell_id,
            "run_index": run_index,
            "host_fingerprint": host_fingerprint,
            "request_sha256": request_sha256,
            "driver_artifact_sha256": physical_artifact_sha256,
        }
    )
    payload = {
        "schema_version": 1,
        "run_id_sha256": run_id,
        "matrix_cell_id": matrix_cell_id,
        "run_index": run_index,
        "host_fingerprint": host_fingerprint,
        "source": manifest["source"],
        "contracts": contracts,
        "package": package,
        "materialized_reused": summary["materialized_reused"],
        "clean": True,
        "unplanned_suspend": False,
        "metrics": raw["metrics"],
    }
    return {
        "run_id_sha256": run_id,
        "matrix_cell_id": matrix_cell_id,
        "run_index": run_index,
        "host_fingerprint": host_fingerprint,
        "clean": True,
        "unplanned_suspend": False,
        "source": manifest["source"],
        "contracts": contracts,
        "package": package,
        "materialized_reused": summary["materialized_reused"],
        "raw_artifact": {
            "name": summary["measurements"]["artifact"],
            "sha256": canonical_sha256(payload),
            "payload": payload,
        },
    }


def collect_constant_calibration(
    args,
    *,
    root: Path,
    unpacked_root: Path,
    cli: Path,
    manifest: dict,
    measurement_contract: dict,
    env: dict[str, str],
    archive_sha256: str,
    package_phase_started: float,
) -> dict:
    protocol = measurement_contract["measurement_protocol"]
    matrix_cell = _constant_calibration_matrix_cell(args, protocol, manifest)
    matrix_cell_id = args.qualification_matrix_cell
    require(
        args.qualification_driver is not None
        and args.qualification_driver.is_file()
        and not args.qualification_driver.is_symlink(),
        "--collect-constant-calibration requires the exact shared calibration driver",
    )
    retained_root = args.constant_calibration_output_dir
    require(
        retained_root is not None,
        "--collect-constant-calibration requires --constant-calibration-output-dir",
    )
    retained_root.mkdir(parents=True, exist_ok=True)
    require(
        retained_root.is_dir()
        and not retained_root.is_symlink()
        and not any(retained_root.iterdir()),
        "constant-calibration output directory must be a new empty directory",
    )
    setup_started = time.perf_counter()
    private_root = root / "constant-calibration"
    private_root.mkdir(mode=0o700)
    project = _prepare_synthetic_project(private_root)
    nonce = secrets.token_hex(32)
    register_failure_evidence_secret(nonce)
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    executable = runtime_executable_path(cli, manifest)
    driver_contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "constant_set_sha256": measurement_contract["constant_set_sha256"],
        "measurement_protocol_sha256": measurement_contract[
            "measurement_protocol_sha256"
        ],
    }
    request = {
        "schema_version": 1,
        "calibration_nonce": nonce,
        "calibration_nonce_sha256": nonce_sha256,
        "source": manifest["source"],
        "package": {
            "archive_sha256": archive_sha256,
            "executable_sha256": runtime_executable_sha256(manifest),
            "asset_target": manifest["asset_target"],
            "release_version": manifest["release_version"],
            "model_sha256": manifest["model"]["sha256"],
        },
        "contracts": driver_contracts,
        "runtime": {
            "engine_policy": "accelerated",
            "expected_backend": normalized_backend(matrix_cell["backend"]),
            "offline": True,
            "matrix_cell_id": matrix_cell_id,
            "cache_state": matrix_cell["cache_state"],
            "residency_state": matrix_cell["residency_state"],
        },
        "project": str(project),
        "required_runs": _RUN_COUNT,
        "output_directory": str(private_root.resolve()),
    }
    request_path = private_root / "request.json"
    output_path = private_root / "output.json"
    write_private_json(request_path, request)
    calibration_env = dict(env)
    require(
        calibration_env.get("CODESTORY_EMBED_ALLOW_CPU") == "0",
        "constant calibration must disable CPU fallback",
    )
    calibration_env["CODESTORY_EMBED_CONSTANT_CALIBRATION_DIR"] = str(
        private_root.resolve()
    )
    calibration_env["CODESTORY_EMBED_CONSTANT_CALIBRATION_NONCE"] = nonce
    calibration_env["CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"] = archive_sha256
    calibration_env["CODESTORY_PLUGIN_CLI_MANIFEST_PATH"] = str(
        _native_manifest_path(unpacked_root)
    )
    setup_finished = time.perf_counter()
    measurement = run(
        [
            str(args.qualification_driver.resolve()),
            "--cli",
            str(executable),
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=calibration_env,
        cwd=root,
        timeout=args.timeout_secs,
    )
    validation_started = time.perf_counter()
    output = _load_json(output_path, "constant-calibration driver output")
    retained = _validate_driver_output(
        output,
        request=request,
        request_path=request_path,
        private_root=private_root,
        protocol=protocol,
        matrix_cell=matrix_cell,
    )
    fingerprint = _host_fingerprint()
    run_artifacts = []
    for summary, raw, physical_digest in retained:
        document = _retained_run(
            summary=summary,
            raw=raw,
            physical_artifact_sha256=physical_digest,
            manifest=manifest,
            measurement_contract=measurement_contract,
            matrix_cell_id=matrix_cell_id,
            matrix_cell=matrix_cell,
            archive_sha256=archive_sha256,
            host_fingerprint=fingerprint,
            request_sha256=output["request_sha256"],
        )
        destination = retained_root / f"run-{summary['run_index']}.json"
        write_json(destination, document)
        run_artifacts.append(
            {
                "name": destination.name,
                "sha256": sha256(destination),
                "raw_artifact_sha256": physical_digest,
            }
        )
    finished = time.perf_counter()
    timing = {
        "schema_version": 1,
        "archive_authentication_unpack_ms": round(
            (setup_started - package_phase_started) * 1000,
            3,
        ),
        "project_and_request_setup_ms": round(
            (setup_finished - setup_started) * 1000,
            3,
        ),
        "measurement_ms": measurement["wall_ms"],
        "retention_validation_ms": round((finished - validation_started) * 1000, 3),
        "end_to_end_ms": round((finished - package_phase_started) * 1000, 3),
    }
    write_json(retained_root / "timing.json", timing)
    return {
        "schema_version": 1,
        "status": "constant_calibration",
        "matrix_cell_id": matrix_cell_id,
        "required_for_assembly": matrix_cell_id in protocol["calibration_matrix"],
        "run_count": len(run_artifacts),
        "metric_count_per_run": _METRIC_COUNT,
        "sample_count_per_metric_per_run": 1,
        "run_artifacts": run_artifacts,
        "timing": timing,
    }
