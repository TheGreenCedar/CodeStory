"""Archive setup and phase orchestration for packaged proof."""

from __future__ import annotations

import argparse
import os
from pathlib import Path

from .archive_io import find_cli, unpack_archive
from .calibration_verification import verify_calibration_bundle
from .contract_primitives import write_json
from .foundation import LEGACY_HELP_TOKENS, REPOSITORY_ROOT, require
from .installation_support import isolated_environment
from .native_manifest import load_native_manifest
from .package_contracts import verify_package_server_contracts
from .qualification_recording import record_qualification_contract
from .runtime_contract import record_runtime_contract, run_runtime_proof
from .server_cleanup import native_server_exit_wait_budget
from .subprocess_control import FailurePreservingTemporaryDirectory, run


def verify_package_source(
    args: argparse.Namespace,
    manifest: dict[str, object],
) -> None:
    if args.expected_source_sha:
        require(
            manifest["source"]["commit"] == args.expected_source_sha,
            "package source commit does not match --expected-source-sha",
        )
    if args.expected_source_tree:
        require(
            manifest["source"]["tree"] == args.expected_source_tree,
            "package source tree does not match --expected-source-tree",
        )


def load_calibration_bundle(
    args: argparse.Namespace,
    manifest: dict[str, object],
    measurement_contract: dict[str, object],
    *,
    required: bool,
) -> dict[str, object] | None:
    if not required:
        require(
            args.calibration_bundle is None
            and args.calibration_producer_run_id is None
            and args.calibration_producer_artifact is None,
            f"{claim_scope(args)} proof rejects calibration inputs",
        )
        return None
    require(
        args.calibration_bundle is not None,
        f"{args.proof_tier} proof requires --calibration-bundle for the frozen constant set",
    )
    require(
        args.calibration_producer_run_id is not None
        and args.calibration_producer_artifact is not None,
        f"{args.proof_tier} proof requires authenticated calibration producer run and artifact identity",
    )
    return verify_calibration_bundle(
        args.calibration_bundle,
        measurement_contract,
        frozen_source=manifest["source"],
        repository_root=REPOSITORY_ROOT,
        enforce_source_lineage=args.enforce_calibration_freeze_lineage,
        expected_producer_run_id=args.calibration_producer_run_id,
        expected_producer_artifact=args.calibration_producer_artifact,
    )


def requires_calibration_bundle(args: argparse.Namespace) -> bool:
    return args.enforce_calibration_freeze_lineage or (
        not args.version_only
        and args.proof_tier != "calibration"
        and not args.server_behavior_only
        and not args.ground_only
    )


def claim_scope(args: argparse.Namespace) -> str:
    if args.ground_only:
        return (
            "installed_ground"
            if args.proof_tier == "installed_runtime"
            else "packaged_ground"
        )
    return "server_behavior_only" if args.server_behavior_only else "qualification"


def package_summary(
    args: argparse.Namespace,
    cli: Path,
    root: Path,
    env: dict[str, str],
    manifest: dict[str, object],
    measurement_contract: dict[str, object],
    calibration_bundle: dict[str, object] | None,
) -> dict[str, object]:
    version = run([str(cli), "--version"], env=env, cwd=root, timeout=args.timeout_secs)
    require(
        args.expected_version in version["stdout"],
        f"CLI version does not contain {args.expected_version}",
    )
    help_result = run(
        [str(cli), "--help"],
        env=env,
        cwd=root,
        timeout=args.timeout_secs,
    )
    require(
        not any(token in help_result["stdout"].lower() for token in LEGACY_HELP_TOKENS),
        "top-level help exposes deleted embedding lifecycle terminology",
    )
    return {
        "version": version,
        "help": help_result,
        "package_contract": {
            "manifest": manifest,
            "answer_quality_claim": False,
            "release_readiness_claim": False,
            "measurement_contract": measurement_contract,
            "calibration_bundle": calibration_bundle,
            "claim_scope": claim_scope(args),
            "highest_proof_tier": "package_structure",
        },
    }


def run_archive_proof(args: argparse.Namespace) -> None:
    temporary_package_directory = FailurePreservingTemporaryDirectory(
        prefix="codestory-packaged-proof-"
    )
    with temporary_package_directory as raw:
        root = Path(raw)
        unpack_archive(args.archive, root / "unpacked")
        cli = find_cli(root / "unpacked")
        manifest = load_native_manifest(
            root / "unpacked",
            cli,
            args.expected_version,
        )
        verify_package_source(args, manifest)
        require_frozen = not args.version_only and args.proof_tier != "calibration"
        require(
            not args.enforce_calibration_freeze_lineage or require_frozen,
            "calibration freeze lineage is valid only for the immediate frozen proof",
        )
        measurement_contract = verify_package_server_contracts(
            manifest,
            args.measurement_protocol,
            require_frozen=require_frozen,
        )
        if args.ground_only and os.name == "nt":
            cleanup_wait_budget = native_server_exit_wait_budget(manifest)
            temporary_package_directory.cleanup_retry_budget_secs = (
                cleanup_wait_budget["timeout_ms"] / 1000
            )
        calibration_bundle = load_calibration_bundle(
            args,
            manifest,
            measurement_contract,
            required=requires_calibration_bundle(args),
        )
        env = isolated_environment(root, args.engine_policy, args.offline)
        summary = package_summary(
            args,
            cli,
            root,
            env,
            manifest,
            measurement_contract,
            calibration_bundle,
        )
        if not args.version_only:
            require(
                args.project is not None,
                "--project is required for the runtime proof",
            )
            require(
                args.engine_policy is not None,
                "--engine-policy is required for the runtime proof",
            )
            runtime = run_runtime_proof(
                args,
                cli,
                env,
                root,
                manifest,
                measurement_contract,
            )
            summary["runtime"] = runtime
            record_runtime_contract(args, summary, manifest, runtime)
            record_qualification_contract(
                args,
                summary,
                manifest,
                runtime,
                measurement_contract,
            )
        write_json(args.out_dir / "summary.json", summary)
