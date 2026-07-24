"""Inputs and external evidence for qualification production."""

from __future__ import annotations

import argparse
import hashlib
import secrets
from dataclasses import dataclass
from pathlib import Path

from .contracts import require_nonempty_string, sha256
from .foundation import RETRIEVAL_QUALITY_EVIDENCE_CONTRACT, ProofFailure, require
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

    @property
    def forbidden_values(self) -> list[str]:
        return [self.nonce, *self.projects]


@dataclass(frozen=True)
class QualificationExternalEvidence:
    publication_fault: dict
    fault_recovery_consistency: dict | None
    retrieval_quality: dict | None


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

