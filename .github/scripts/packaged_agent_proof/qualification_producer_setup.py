"""Qualification producer setup and external evidence collection."""

from __future__ import annotations

import argparse
import hashlib
import secrets
from pathlib import Path

from .contract_primitives import sha256
from .failure_evidence import register_failure_evidence_secret
from .foundation import require
from .native_manifest import runtime_executable_sha256
from .publication_consistency_verifier import (
    verify_fault_recovery_consistency_raw_evidence,
)
from .publication_fault_producer import produce_product_publication_fault_evidence
from .publication_fault_verifier import verify_publication_fault_raw_evidence
from .qualification_directory_binding import bind_qualification_directory
from .qualification_production_types import (
    QualificationExternalEvidence,
    QualificationProducerContext,
)


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
        args.qualification_driver is not None
        and args.qualification_driver.is_file(),
        "--produce-qualification-evidence requires an existing --qualification-driver",
    )
    require(
        qualification_cli.is_file(),
        f"qualification executable is missing: {qualification_cli}",
    )
    require(
        sha256(qualification_cli) == runtime_executable_sha256(manifest),
        "qualification executable does not match the packaged runtime executable",
    )
    private_root = root / "qualification-suite"
    artifact_root = private_root / "artifacts"
    private_root.mkdir(mode=0o700)
    artifact_root.mkdir(mode=0o700)
    nonce = secrets.token_hex(32)
    register_failure_evidence_secret(nonce)
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
        "executable_sha256": runtime_executable_sha256(manifest),
        "asset_target": manifest["asset_target"],
        "release_version": manifest["release_version"],
    }
    qualification_env = dict(env)
    qualification_env.pop("CODESTORY_CLI", None)
    # Nothing is resident on this directory yet, so the initial bind is the one
    # binding that needs no server replacement. Every later move is a rebind.
    bind_qualification_directory(qualification_env, private_root)
    qualification_env["CODESTORY_EMBED_QUALIFICATION_NONCE"] = nonce
    qualification_env["CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"] = archive_sha256
    server_cleanup_control.update(
        {
            "qualification_cli": str(qualification_cli.resolve()),
            "qualification_directory": str(private_root.resolve()),
            "qualification_nonce": nonce,
            "plugin_cli_archive_sha256": archive_sha256,
            "plugin_cli_manifest_path": qualification_env.get(
                "CODESTORY_PLUGIN_CLI_MANIFEST_PATH"
            ),
            "projects": list(projects),
        }
    )
    return QualificationProducerContext(
        args=args,
        qualification_driver=args.qualification_driver,
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
    return QualificationExternalEvidence(
        publication_fault,
        consistency,
    )
