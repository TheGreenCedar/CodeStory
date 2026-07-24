"""Inputs and external evidence for qualification production."""

from __future__ import annotations

import argparse
import hashlib
import json
import secrets
from dataclasses import dataclass
from pathlib import Path

from .contracts import (
    require_exact_keys,
    require_nonempty_string,
    selected_qualification_matrix_cell,
    sha256,
    write_private_json,
)
from .foundation import (
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    ProofFailure,
    require,
)
from .process import run
from .runtime import (
    produce_product_publication_fault_evidence,
    verify_fault_recovery_consistency_raw_evidence,
    verify_publication_fault_raw_evidence,
    verify_retrieval_quality_raw_evidence,
)


@dataclass(frozen=True)
class QualificationProducerContext:
    args: argparse.Namespace
    qualification_cli: Path
    root: Path
    runtime: dict
    manifest: dict
    archive_sha256: str
    measurement_contract: dict
    private_root: Path
    artifact_root: Path
    nonce: str
    nonce_sha256: str
    projects: tuple[str, str]
    contracts: dict
    package: dict
    qualification_env: dict[str, str]
    server_cleanup_control: dict

    @property
    def forbidden_values(self) -> list[str]:
        return [self.nonce, *self.projects]


@dataclass(frozen=True)
class QualificationExternalEvidence:
    publication_fault: dict
    fault_recovery_consistency: dict | None
    retrieval_quality: dict | None


@dataclass(frozen=True)
class QualificationRunnerEvidence:
    output: dict
    expected_status: str
    expected_backend: str
    matrix_cell_id: str
    matrix_cell: dict


def prepare_qualification_producer(
    args: argparse.Namespace,
    qualification_cli: Path,
    env: dict[str, str],
    root: Path,
    runtime: dict,
    manifest: dict,
    archive_sha256: str,
    measurement_contract: dict,
    server_cleanup_control: dict,
) -> QualificationProducerContext:
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
    projects = runtime.get("_qualification_projects")
    require(
        isinstance(projects, list)
        and len(projects) == 2
        and all(
            isinstance(project, str) and Path(project).is_absolute()
            for project in projects
        ),
        "runtime proof omitted its two qualification projects",
    )
    contracts = {
        "protocol_sha256": measurement_contract["protocol_sha256"],
        "constant_set_sha256": measurement_contract["constant_set_sha256"],
        "measurement_protocol_sha256": measurement_contract[
            "measurement_protocol_sha256"
        ],
    }
    package = {
        "archive_sha256": archive_sha256,
        "executable_sha256": manifest["binary"]["sha256"],
        "asset_target": manifest["asset_target"],
        "release_version": manifest["release_version"],
    }
    qualification_env = dict(env)
    qualification_env.pop("CODESTORY_CLI", None)
    qualification_env["CODESTORY_EMBED_QUALIFICATION_DIR"] = str(
        private_root.resolve()
    )
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
    return QualificationProducerContext(
        args=args,
        qualification_cli=qualification_cli,
        root=root,
        runtime=runtime,
        manifest=manifest,
        archive_sha256=archive_sha256,
        measurement_contract=measurement_contract,
        private_root=private_root,
        artifact_root=artifact_root,
        nonce=nonce,
        nonce_sha256=hashlib.sha256(nonce.encode("ascii")).hexdigest(),
        projects=(projects[0], projects[1]),
        contracts=contracts,
        package=package,
        qualification_env=qualification_env,
        server_cleanup_control=server_cleanup_control,
    )


def collect_qualification_external_evidence(
    context: QualificationProducerContext,
) -> QualificationExternalEvidence:
    args = context.args
    consistency = None
    if args.publication_fault_evidence is None:
        (
            args.publication_fault_evidence,
            consistency_path,
        ) = produce_product_publication_fault_evidence(
            context.qualification_cli,
            context.qualification_env,
            context.private_root,
            context.artifact_root,
            context.nonce,
            source=context.manifest["source"],
            package=context.package,
            contracts=context.contracts,
            timeout=args.timeout_secs,
        )
        consistency = verify_fault_recovery_consistency_raw_evidence(
            consistency_path,
            source=context.manifest["source"],
            package=context.package,
            contracts=context.contracts,
        )
    publication_fault = verify_publication_fault_raw_evidence(
        args.publication_fault_evidence,
        source=context.manifest["source"],
        package=context.package,
        contracts=context.contracts,
    )
    retrieval_quality = None
    if args.retrieval_quality_evidence is not None:
        retrieval_quality = verify_retrieval_quality_raw_evidence(
            args.retrieval_quality_evidence,
            source=context.manifest["source"],
        )
    elif args.proof_tier != "calibration":
        raise ProofFailure(
            f"{args.proof_tier} qualification requires "
            "--retrieval-quality-evidence "
            f"from {RETRIEVAL_QUALITY_EVIDENCE_CONTRACT}"
        )
    return QualificationExternalEvidence(
        publication_fault,
        consistency,
        retrieval_quality,
    )


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
