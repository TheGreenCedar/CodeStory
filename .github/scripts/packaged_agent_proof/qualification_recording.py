"""Retained qualification selection and recording for packaged proof."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from .contract_primitives import require_nonempty_string, sha256
from .foundation import ProofFailure, require
from .measurement_samples import selected_qualification_matrix_cell
from .qualification_retained import verify_retained_qualification
from .runtime_contract import installed_runtime_provenance_is_proven


def load_evidence(path: Path, label: str) -> dict[str, object]:
    try:
        evidence = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(evidence, dict), f"{label} must be an object")
    return evidence


def record_retained_qualification(
    args: argparse.Namespace,
    summary: dict[str, object],
    manifest: dict[str, object],
    runtime: dict[str, object],
    measurement_contract: dict[str, object],
) -> None:
    require(
        args.qualification_evidence is not None,
        f"{args.proof_tier} proof requires --qualification-evidence from the exact live scenario run",
    )
    retained = load_evidence(args.qualification_evidence, "qualification evidence")
    matrix_cell_id = require_nonempty_string(
        args.qualification_matrix_cell,
        f"{args.proof_tier} proof requires --qualification-matrix-cell",
    )
    backend = args.expected_backend or require_nonempty_string(
        runtime["identity"].get("embedding_backend"),
        "runtime embedding backend",
    )
    matrix_cell = selected_qualification_matrix_cell(
        measurement_contract["measurement_protocol"],
        cell_id=matrix_cell_id,
        target=manifest["asset_target"],
        proof_tier=args.proof_tier,
        expected_policy=args.engine_policy,
        expected_backend=backend,
    )
    summary["qualification"] = verify_retained_qualification(
        retained,
        manifest=manifest,
        archive_sha256=sha256(args.archive),
        shared_identity=runtime["shared_identity"],
        measurement_contract=measurement_contract,
        required_tier=args.proof_tier,
        required_matrix_cell_id=matrix_cell_id,
        expected_policy=args.engine_policy,
        expected_backend=backend,
        expected_accelerator_claim=matrix_cell["accelerator_claim"],
        installed_plugin=runtime.get("installed_plugin"),
        managed_runtime=runtime.get("managed_runtime"),
    )
    summary["package_contract"]["highest_proof_tier"] = args.proof_tier


def record_calibration_qualification(
    args: argparse.Namespace,
    summary: dict[str, object],
) -> None:
    if args.qualification_evidence is None or not args.qualification_evidence.is_file():
        return
    calibration = load_evidence(
        args.qualification_evidence,
        "calibration evidence",
    )
    require(
        calibration.get("schema_version") == 1
        and calibration.get("status") == "calibration"
        and calibration.get("tier") == "calibration",
        "calibration evidence has the wrong schema, status, or tier",
    )
    summary["qualification"] = calibration


def record_qualification_contract(
    args: argparse.Namespace,
    summary: dict[str, object],
    manifest: dict[str, object],
    runtime: dict[str, object],
    measurement_contract: dict[str, object],
) -> None:
    if args.ground_only:
        return
    if args.server_behavior_only:
        summary["server_behavior"] = {
            "status": "pass",
            "runtime_tier_exercised": args.proof_tier,
            "answer_quality_claim": False,
            "retrieval_quality_claim": False,
            "release_readiness_claim": True,
            "installed_runtime_provenance_proven": (
                installed_runtime_provenance_is_proven(args, runtime)
            ),
        }
        summary["package_contract"]["release_readiness_claim"] = True
        summary["package_contract"]["highest_proof_tier"] = "server_behavior"
    elif args.proof_tier == "calibration":
        record_calibration_qualification(args, summary)
    else:
        record_retained_qualification(
            args,
            summary,
            manifest,
            runtime,
            measurement_contract,
        )
