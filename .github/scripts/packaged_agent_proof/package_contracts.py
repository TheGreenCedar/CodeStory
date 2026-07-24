"""Packaged server contract verification."""

from __future__ import annotations

import re
from pathlib import Path

from .foundation import SERVER_LIFECYCLES, require
from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_sha256,
)
from .measurement_protocol import load_server_measurement_contract


def _verify_frozen_constant_set(measurement: dict, constant_set: dict) -> None:
    require(
        constant_set.get("status") == "frozen",
        "embedding server constants are not frozen; calibration cannot be treated as qualification",
    )
    freeze_record = constant_set.get("freeze_record")
    require(isinstance(freeze_record, dict), "frozen embedding server constants omit their freeze record")
    require_exact_keys(
        freeze_record,
        {
            "selection_source_commit",
            "selection_source_tree",
            "measurement_protocol_sha256",
            "protocol_sha256",
            "input_constant_set_sha256",
            "calibration_bundle_sha256",
            "calibration_freeze_digest",
            "run_artifact_sha256s",
            "selection_rule",
            "selected_at",
        },
        "constant-set freeze_record",
    )
    for field in (
        "selection_source_commit",
        "selection_source_tree",
        "measurement_protocol_sha256",
        "protocol_sha256",
        "input_constant_set_sha256",
        "calibration_bundle_sha256",
        "calibration_freeze_digest",
        "selection_rule",
        "selected_at",
    ):
        require_nonempty_string(
            freeze_record.get(field),
            f"constant-set freeze_record.{field}",
        )
    for field in ("selection_source_commit", "selection_source_tree"):
        require(
            re.fullmatch(r"[0-9a-f]{40}", freeze_record[field]) is not None,
            f"constant-set freeze_record.{field} must be a lowercase Git object id",
        )
    for field in (
        "measurement_protocol_sha256",
        "protocol_sha256",
        "input_constant_set_sha256",
        "calibration_bundle_sha256",
        "calibration_freeze_digest",
    ):
        require_sha256(freeze_record[field], f"constant-set freeze_record.{field}")
    run_digests = freeze_record["run_artifact_sha256s"]
    required_run_count = len(measurement["calibration_matrix"]) * 3
    require(
        isinstance(run_digests, list)
        and len(run_digests) == required_run_count
        and len(set(run_digests)) == required_run_count,
        "constant-set freeze record must bind three distinct runs for every calibration cell",
    )
    for index, digest in enumerate(run_digests):
        require_sha256(digest, f"constant-set freeze_record.run_artifact_sha256s[{index}]")
    unresolved = [
        field
        for section in ("calibration_required_values", "qualification_thresholds")
        for field, value in constant_set.get(section, {}).items()
        if value is None
    ]
    require(not unresolved, "frozen embedding server constants contain unresolved values: " + ", ".join(unresolved))


def verify_package_server_contracts(
    manifest: dict,
    measurement_protocol_path: Path,
    *,
    require_frozen: bool,
) -> dict:
    contract = load_server_measurement_contract(measurement_protocol_path)
    measurement = contract["measurement_protocol"]
    measurement_sha256 = contract["measurement_protocol_sha256"]
    protocol = contract["protocol"]
    protocol_sha256 = contract["protocol_sha256"]
    constant_set = contract["constant_set"]
    constant_set_sha256 = contract["constant_set_sha256"]
    server_proof = manifest.get("server_proof")
    require(isinstance(server_proof, dict), "package manifest omitted server_proof")
    expected = {
        "measurement_protocol_sha256": measurement_sha256,
        "protocol_sha256": protocol_sha256,
        "constant_set_sha256": constant_set_sha256,
    }
    for field, digest in expected.items():
        require(
            server_proof.get(field) == digest,
            f"package manifest {field} does not match the checked-in contract",
        )
    require(
        server_proof.get("constant_set_status") == constant_set.get("status"),
        "package manifest constant-set status does not match the checked-in contract",
    )
    require(
        set(protocol.get("lifecycle_states", [])) == SERVER_LIFECYCLES,
        "embedding server lifecycle states do not match the verifier",
    )
    required_metrics = set(measurement["required_metrics"])
    thresholds = constant_set.get("qualification_thresholds")
    require(
        isinstance(thresholds, dict) and set(thresholds) == required_metrics,
        "embedding server qualification thresholds do not match the measurement metrics",
    )
    if require_frozen:
        _verify_frozen_constant_set(measurement, constant_set)
    return contract
