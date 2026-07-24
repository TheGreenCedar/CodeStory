"""Qualification subprocess request, execution, and output validation."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from .contracts import require_exact_keys, require_nonempty_string, selected_qualification_matrix_cell, write_private_json
from .foundation import REQUIRED_SERVER_SCENARIOS, ProofFailure, require
from .process import run
from .qualification_production_types import QualificationProducerContext, QualificationRunnerEvidence

def _qualification_request(
    context: QualificationProducerContext,
    *,
    expected_backend: str,
    matrix_cell_id: str,
    matrix_cell: dict,
) -> dict:
    return {
        "schema_version": 1,
        "qualification_nonce": context.nonce,
        "qualification_nonce_sha256": context.nonce_sha256,
        "proof_tier": context.args.proof_tier,
        "source": context.manifest["source"],
        "package": context.package,
        "contracts": context.contracts,
        "runtime": {
            "engine_policy": context.args.engine_policy,
            "expected_backend": expected_backend,
            "offline": context.args.offline,
            "matrix_cell_id": matrix_cell_id,
            "cache_state": matrix_cell["cache_state"],
            "residency_state": matrix_cell["residency_state"],
        },
        "projects": list(context.projects),
        "required_scenarios": sorted(REQUIRED_SERVER_SCENARIOS),
        "required_metrics": sorted(
            context.measurement_contract["measurement_protocol"][
                "required_metrics"
            ]
        ),
        "output_directory": str(context.artifact_root.resolve()),
    }


def _validated_qualification_output(
    context: QualificationProducerContext,
    *,
    request: dict,
    request_path: Path,
    output_path: Path,
) -> dict:
    require(
        output_path.is_file() and not output_path.is_symlink(),
        "qualification runner omitted its output",
    )
    output_bytes = output_path.read_bytes()
    for forbidden in context.forbidden_values:
        require(
            forbidden.encode("utf-8") not in output_bytes,
            "qualification runner output leaked private request material",
        )
    try:
        output = json.loads(output_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"qualification runner output is not valid JSON: {exc}"
        ) from exc
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
    require(
        output["schema_version"] == 2,
        "qualification runner schema is unsupported",
    )
    require(
        output["tier"] == context.args.proof_tier,
        "qualification runner returned the wrong proof tier",
    )
    require(
        output["source"] == context.manifest["source"],
        "qualification runner source identity is stale",
    )
    require(
        output["package"] == context.package,
        "qualification runner package identity is stale",
    )
    require(
        output["contracts"] == context.contracts,
        "qualification runner contract identity is stale",
    )
    require(
        output["runtime"] == request["runtime"],
        "qualification runner runtime identity is stale",
    )
    require(
        output["request_sha256"]
        == hashlib.sha256(request_path.read_bytes()).hexdigest(),
        "qualification runner output is not bound to the exact private request",
    )
    return output


def run_qualification_producer(
    context: QualificationProducerContext,
) -> QualificationRunnerEvidence:
    identity = context.runtime["identity"]
    expected_backend = context.args.expected_backend or require_nonempty_string(
        identity.get("embedding_backend"),
        "runtime embedding backend",
    )
    matrix_cell_id = require_nonempty_string(
        context.args.qualification_matrix_cell,
        "--produce-qualification-evidence requires --qualification-matrix-cell",
    )
    matrix_cell = selected_qualification_matrix_cell(
        context.measurement_contract["measurement_protocol"],
        cell_id=matrix_cell_id,
        target=context.manifest["asset_target"],
        proof_tier=context.args.proof_tier,
        expected_policy=context.args.engine_policy,
        expected_backend=expected_backend,
    )
    request = _qualification_request(
        context,
        expected_backend=expected_backend,
        matrix_cell_id=matrix_cell_id,
        matrix_cell=matrix_cell,
    )
    artifact_root = str(context.artifact_root.resolve())
    context.qualification_env["CODESTORY_EMBED_QUALIFICATION_DIR"] = artifact_root
    context.server_cleanup_control["qualification_directory"] = artifact_root
    request_path = context.artifact_root / "request.json"
    output_path = context.artifact_root / "output.json"
    write_private_json(request_path, request)
    run(
        [
            str(context.qualification_cli),
            "internal-embedding-qualification",
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=context.qualification_env,
        cwd=context.root,
        timeout=context.args.timeout_secs,
    )
    output = _validated_qualification_output(
        context,
        request=request,
        request_path=request_path,
        output_path=output_path,
    )
    expected_status = (
        "calibration" if context.args.proof_tier == "calibration" else "pass"
    )
    return QualificationRunnerEvidence(
        output,
        expected_status,
        expected_backend,
        matrix_cell_id,
        matrix_cell,
    )
