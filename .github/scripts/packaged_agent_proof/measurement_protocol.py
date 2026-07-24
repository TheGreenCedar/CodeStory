"""Measurement protocol and holdout-contract loading."""

from __future__ import annotations

import json
import re
from pathlib import Path

from .contract_primitives import canonical_sha256, require_nonempty_string, sha256
from .foundation import HOLDOUT_TASK_ROOT, REPOSITORY_ROOT, REQUIRED_HOLDOUT_TASK_FILES, SERVER_CONSTANT_SET, SERVER_PROTOCOL, ProofFailure, require
from .measurement_constant_selection import _verify_constant_selection, _verify_thresholds_and_clock
from .measurement_protocol_validation import _measurement_document, _verify_measurement_matrices, _verify_measurement_sampling, _verify_scenario_and_metric_contracts

def load_measurement_protocol(path: Path) -> tuple[dict, str]:
    protocol = _measurement_document(path)
    required_metrics, metric_contracts = _verify_scenario_and_metric_contracts(
        protocol
    )
    _verify_measurement_matrices(protocol)
    _verify_measurement_sampling(protocol, required_metrics, metric_contracts)
    _verify_constant_selection(protocol)
    _verify_thresholds_and_clock(protocol)
    return protocol, sha256(path)


def load_json_contract(path: Path, label: str) -> tuple[dict, str]:
    require(path.is_file(), f"{label} is missing: {path}")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(document, dict), f"{label} must be an object")
    require(document.get("schema_version") == 1, f"{label} schema is unsupported")
    return document, sha256(path)


def load_server_measurement_contract(measurement_protocol_path: Path) -> dict:
    measurement, measurement_sha256 = load_measurement_protocol(
        measurement_protocol_path
    )
    protocol_path = measurement_protocol_path.with_name(SERVER_PROTOCOL.name)
    constant_set_path = measurement_protocol_path.with_name(SERVER_CONSTANT_SET.name)
    protocol, protocol_sha256 = load_json_contract(
        protocol_path, "embedding server protocol"
    )
    constant_set, constant_set_sha256 = load_json_contract(
        constant_set_path,
        "embedding server constant set",
    )
    return {
        "measurement_protocol": measurement,
        "measurement_protocol_sha256": measurement_sha256,
        "protocol": protocol,
        "protocol_sha256": protocol_sha256,
        "constant_set": constant_set,
        "constant_set_sha256": constant_set_sha256,
    }


def load_holdout_task_contracts(root: Path = HOLDOUT_TASK_ROOT) -> tuple[dict[tuple[str, str], dict], str]:
    require(root.is_dir(), f"holdout retrieval task directory is missing: {root}")
    paths = sorted(root.glob("*.task.json"))
    require(
        {path.name for path in paths} == REQUIRED_HOLDOUT_TASK_FILES,
        "checked-in holdout retrieval task set changed without updating the release contract",
    )
    tasks: dict[tuple[str, str], dict] = {}
    corpus_records = []
    for path in paths:
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise ProofFailure(f"holdout task manifest is not valid JSON: {path}: {exc}") from exc
        require(isinstance(raw, dict), f"holdout task manifest must be an object: {path}")
        require(raw.get("version") == 1, f"holdout task manifest schema is unsupported: {path}")
        task_id = require_nonempty_string(raw.get("id"), f"holdout task {path.name}.id")
        require(
            path.name == f"{task_id}.task.json",
            f"holdout task id does not match its checked-in filename: {path}",
        )
        require(raw.get("suite") == "holdout-retrieval", f"holdout task {task_id} left the release suite")
        repo = raw.get("repo")
        require(isinstance(repo, dict), f"holdout task {task_id} omitted repository identity")
        repo_name = require_nonempty_string(repo.get("name"), f"holdout task {task_id} repo.name")
        require(
            isinstance(repo.get("url"), str)
            and re.fullmatch(
                r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?",
                repo["url"],
            )
            is not None,
            f"holdout task {task_id} repository URL is not trusted",
        )
        require(
            isinstance(repo.get("ref"), str)
            and re.fullmatch(r"[0-9a-f]{40}", repo["ref"]) is not None,
            f"holdout task {task_id} repository ref is not immutable",
        )
        key = (repo_name, task_id)
        require(key not in tasks, f"holdout task identity is duplicated: {repo_name}/{task_id}")
        expected_snapshot = {
            "id": task_id,
            "name": raw.get("name", task_id),
            "suite": "holdout-retrieval",
            "repo": repo_name,
            "repo_metadata": repo,
            "task_class": raw.get("task_class"),
            "prompt": raw.get("prompt"),
            "expected_files": raw.get("expected_files", []),
            "expected_verification_files": raw.get("expected_verification_files", []),
            "expected_symbols": raw.get("expected_symbols", []),
            "expected_symbol_probes": raw.get("expected_symbol_probes", []),
            "expected_claims": raw.get("expected_claims", []),
            "forbidden_claims": raw.get("forbidden_claims", []),
            "quality_thresholds": raw.get("quality_thresholds", {}),
        }
        tasks[key] = {
            "path": path,
            "manifest_sha256": sha256(path),
            "snapshot": expected_snapshot,
            "repo": repo,
        }
        corpus_records.append(
            {
                "path": path.relative_to(REPOSITORY_ROOT).as_posix(),
                "sha256": tasks[key]["manifest_sha256"],
            }
        )
    return tasks, canonical_sha256(corpus_records)
