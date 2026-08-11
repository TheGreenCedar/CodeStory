"""Frozen-candidate calibration acceptance and artifact binding."""

from __future__ import annotations

import json
import re
from pathlib import Path

from .calibration_lineage import verify_release_head_calibration_lineage
from .calibration_verification import verify_calibration_bundle
from .contract_primitives import sha256
from .foundation import ProofFailure, require
from .measurement_protocol import load_server_measurement_contract
from .package_contracts import _verify_frozen_constant_set

MEASUREMENT_PROTOCOL_PATH = Path(
    "crates/codestory-llama-sys/per-user-embedding-server-measurement-protocol.json"
)
SELECTED_AT_PATTERN = re.compile(r"github-actions-run:([1-9][0-9]*):([1-9][0-9]*)")


def resolve_frozen_acceptance_identity(measurement_contract: dict) -> dict[str, str]:
    """Resolve the immutable producer coordinates recorded by calibration."""

    constant_set = measurement_contract["constant_set"]
    _verify_frozen_constant_set(
        measurement_contract["measurement_protocol"],
        constant_set,
    )
    freeze_record = constant_set["freeze_record"]
    selected_at = freeze_record["selected_at"]
    match = SELECTED_AT_PATTERN.fullmatch(selected_at)
    require(
        match is not None,
        "constant-set freeze_record.selected_at must name one exact GitHub Actions "
        "run and attempt",
    )
    source_commit = freeze_record["selection_source_commit"]
    return {
        "source_commit": source_commit,
        "producer_run_id": match.group(1),
        "producer_run_attempt": match.group(2),
        "artifact_name": f"embedding-calibration-bundle-{source_commit}",
    }


def require_frozen_acceptance_coordinates(
    identity: dict[str, str],
    *,
    producer_run_id: str,
    producer_run_attempt: str,
    producer_artifact: str,
) -> None:
    require(
        producer_run_id == identity["producer_run_id"]
        and producer_run_attempt == identity["producer_run_attempt"]
        and producer_artifact == identity["artifact_name"],
        "calibration download coordinates differ from the frozen constant-set record",
    )


def _load_json_object(path: Path, label: str) -> dict:
    require(
        path.is_file() and not path.is_symlink(),
        f"{label} is missing or unsafe: {path}",
    )
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(value, dict), f"{label} must be an object")
    return value


def require_artifact_constant_set_matches(
    checked_in_constant_set: dict,
    artifact_constant_set: dict,
) -> None:
    require(
        artifact_constant_set == checked_in_constant_set,
        "downloaded calibration artifact constant set differs from the checked-in "
        "frozen constant set",
    )


def require_artifact_constant_set_digest(
    checked_in_constant_set_sha256: str,
    artifact_constant_set_path: Path,
) -> None:
    require(
        sha256(artifact_constant_set_path) == checked_in_constant_set_sha256,
        "downloaded calibration artifact constant-set bytes differ from the "
        "checked-in frozen constant set",
    )


def verify_frozen_candidate_acceptance(
    repository_root: Path,
    expected_head_sha: str,
    *,
    calibration_bundle: Path,
    artifact_constant_set: Path,
    producer_run_id: str,
    producer_run_attempt: str,
    producer_artifact: str,
) -> dict:
    """Verify Git lineage and mechanically replay the calibration selection."""

    measurement_contract = load_server_measurement_contract(
        repository_root / MEASUREMENT_PROTOCOL_PATH
    )
    identity = resolve_frozen_acceptance_identity(measurement_contract)
    require_frozen_acceptance_coordinates(
        identity,
        producer_run_id=producer_run_id,
        producer_run_attempt=producer_run_attempt,
        producer_artifact=producer_artifact,
    )
    lineage = verify_release_head_calibration_lineage(
        repository_root,
        expected_head_sha,
    )
    verification = verify_calibration_bundle(
        calibration_bundle,
        measurement_contract,
        enforce_source_lineage=False,
        expected_producer_run_id=producer_run_id,
        expected_producer_artifact=producer_artifact,
    )
    require(
        verification["source"]["commit"] == identity["source_commit"]
        and verification["producer"]["run_attempt"] == producer_run_attempt,
        "calibration bundle source or run attempt differs from the frozen "
        "constant-set record",
    )
    artifact_constant = _load_json_object(
        artifact_constant_set,
        "downloaded calibration artifact constant set",
    )
    require_artifact_constant_set_digest(
        measurement_contract["constant_set_sha256"],
        artifact_constant_set,
    )
    require_artifact_constant_set_matches(
        measurement_contract["constant_set"],
        artifact_constant,
    )
    return {
        "lineage": lineage,
        "identity": identity,
        "calibration": verification,
    }
