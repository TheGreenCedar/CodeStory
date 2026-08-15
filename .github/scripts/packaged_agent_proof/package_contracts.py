"""Packaged server contract verification."""

from __future__ import annotations

import re
from pathlib import Path

from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
    require_sha256,
)
from .foundation import SERVER_LIFECYCLES, require
from .measurement_protocol import load_server_measurement_contract
from .qualification_thresholds import verify_qualification_threshold_contract


def _verify_frozen_constant_set(measurement: dict, constant_set: dict) -> None:
    require(
        constant_set.get("status") == "frozen",
        "embedding server constants are not frozen; calibration cannot be treated as qualification",
    )
    freeze_record = constant_set.get("freeze_record")
    require(
        isinstance(freeze_record, dict),
        "frozen embedding server constants omit their freeze record",
    )
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
        require_sha256(
            digest, f"constant-set freeze_record.run_artifact_sha256s[{index}]"
        )
    unresolved = [
        field
        for section in ("calibration_required_values", "qualification_thresholds")
        for field, value in constant_set.get(section, {}).items()
        if value is None
    ]
    require(
        not unresolved,
        "frozen embedding server constants contain unresolved values: "
        + ", ".join(unresolved),
    )


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
    verify_qualification_threshold_contract(
        constant_set,
        required_metrics,
        measurement,
    )
    thresholds = constant_set["qualification_thresholds"]
    fixed = constant_set.get("fixed_contract_values")
    threshold_contract = measurement.get("qualification_threshold_contract", {}).get(
        "true_idle_exit"
    )
    require(
        isinstance(fixed, dict)
        and isinstance(threshold_contract, dict)
        and require_positive_int(
            fixed.get("idle_timeout_ms"),
            "fixed per-user embedding idle timeout",
        )
        == threshold_contract["idle_timeout_ms"]
        and require_positive_int(
            fixed.get("true_idle_observation_grace_ms"),
            "fixed true-idle observation grace",
        )
        == threshold_contract["observation_grace_ms"]
        and thresholds.get("true_idle_exit")
        == threshold_contract["required_threshold_ms"]
        == (
            threshold_contract["idle_timeout_ms"]
            + threshold_contract["observation_grace_ms"]
        ),
        "true-idle qualification threshold must be the fixed product timeout plus observation grace",
    )
    if require_frozen:
        _verify_frozen_constant_set(measurement, constant_set)
    return contract
