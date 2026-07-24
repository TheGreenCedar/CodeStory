"""Runtime execution and retained contract recording for packaged proof."""

from __future__ import annotations

import argparse
from pathlib import Path

from .contract_primitives import require_nonempty_string, sha256
from .foundation import require
from .ground_proof import prove_ground_only_runtime
from .native_contract_identity import verify_runtime_against_manifest
from .qualification_workflow import produce_qualification_evidence
from .runtime_bootstrap import prove_runtime
from .server_cleanup import wait_for_final_temporary_package_server
from .subprocess_control import add_exception_note


def run_runtime_proof(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    manifest: dict[str, object],
    measurement_contract: dict[str, object],
) -> dict[str, object]:
    server_cleanup_control = {"_waiters": []}
    runtime = None
    runtime_error = None
    runtime_traceback = None
    try:
        if args.ground_only:
            runtime = prove_ground_only_runtime(
                args,
                cli,
                env,
                root,
                args.out_dir,
                manifest,
            )
        else:
            runtime = prove_runtime(
                args,
                cli,
                env,
                root,
                args.out_dir,
                manifest,
                server_cleanup_control,
            )
        if args.produce_qualification_evidence:
            qualification_cli = Path(
                require_nonempty_string(
                    runtime.get("_qualification_cli_path"),
                    "runtime qualification executable",
                )
            )
            produce_qualification_evidence(
                args,
                qualification_cli,
                env,
                root,
                runtime,
                manifest,
                sha256(args.archive),
                measurement_contract,
                server_cleanup_control,
            )
    except BaseException as error:
        runtime_error = error
        runtime_traceback = error.__traceback__

    cleanup = None
    cleanup_error = None
    try:
        cleanup = wait_for_final_temporary_package_server(
            args,
            env,
            server_cleanup_control,
            manifest,
            require_final_server=runtime_error is None and not args.ground_only,
        )
    except BaseException as error:
        cleanup_error = error
    if runtime_error is not None:
        if cleanup_error is not None:
            add_exception_note(
                runtime_error,
                f"final temporary package cleanup also failed: {cleanup_error}",
            )
        raise runtime_error.with_traceback(runtime_traceback)
    if cleanup_error is not None:
        raise cleanup_error

    require(isinstance(runtime, dict), "runtime proof returned no evidence")
    if cleanup is not None:
        runtime["temporary_package_cleanup"] = cleanup
    for field in (
        "_qualification_cli_path",
        "_qualification_projects",
        "_memory_observations",
        "_qualification_forbidden_values",
    ):
        runtime.pop(field, None)
    return runtime


def installed_runtime_provenance_is_proven(
    args: argparse.Namespace,
    runtime: dict[str, object],
) -> bool:
    return (
        args.proof_tier == "installed_runtime"
        and isinstance(runtime.get("installed_plugin"), dict)
        and isinstance(runtime.get("managed_runtime"), dict)
    )


def record_runtime_contract(
    args: argparse.Namespace,
    summary: dict[str, object],
    manifest: dict[str, object],
    runtime: dict[str, object],
) -> None:
    package_contract = summary["package_contract"]
    if args.ground_only:
        installed_provenance = installed_runtime_provenance_is_proven(args, runtime)
        if args.proof_tier == "installed_runtime":
            require(
                installed_provenance,
                "installed ground proof omitted exact plugin or managed runtime provenance",
            )
        summary["ground_receipt"] = {
            "status": "pass",
            "runtime_tier_exercised": args.proof_tier,
            "project_bound": runtime["ground"]["project_bound"],
            "answer_quality_claim": False,
            "retrieval_quality_claim": False,
            "shared_server_claim": False,
            "accelerator_claim": False,
            "installed_runtime_provenance_proven": installed_provenance,
        }
        package_contract["highest_proof_tier"] = (
            "installed_ground"
            if args.proof_tier == "installed_runtime"
            else "packaged_ground"
        )
        return
    package_contract["runtime_evidence"] = verify_runtime_against_manifest(
        manifest,
        runtime,
        args.engine_policy,
    )
    package_contract["highest_proof_tier"] = (
        "calibration" if args.proof_tier == "calibration" else "hosted_package"
    )
