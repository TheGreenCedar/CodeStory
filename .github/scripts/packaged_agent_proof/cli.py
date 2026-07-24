"""Cli for packaged CodeStory proof."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from .foundation import (
    DEFAULT_QUERY,
    DEFAULT_QUESTION,
    LEGACY_HELP_TOKENS,
    MEASUREMENT_PROTOCOL,
    REPOSITORY_ROOT,
    ProofFailure,
    require,
)
from .contracts import (
    require_nonempty_string,
    selected_qualification_matrix_cell,
    sha256,
    validate_runtime_claim_scope,
    verify_package_server_contracts,
    write_json,
)
from .archive import (
    expected_archive_digest,
    find_cli,
    load_native_manifest,
    unpack_archive,
    verify_runtime_against_manifest,
)
from .process import (
    FailurePreservingTemporaryDirectory,
    add_exception_note,
    native_server_exit_wait_budget,
    run,
    wait_for_final_temporary_package_server,
)
from .installation import (
    isolated_environment,
    prepare_candidate_installed_proof,
    prove_ground_only_runtime,
)
from .runtime import (
    prove_runtime,
)
from .qualification import (
    require_candidate_matrix_installation_source,
    verify_retained_qualification,
)
from .calibration import (
    assemble_calibration_bundle,
    produce_qualification_evidence,
    verify_calibration_bundle,
)
def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--checksum-file", type=Path)
    parser.add_argument("--expected-version")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--plugin-root", type=Path)
    parser.add_argument("--out-dir", type=Path, default=Path("target/packaged-agent-proof"))
    parser.add_argument("--query", default=DEFAULT_QUERY)
    parser.add_argument("--question", default=DEFAULT_QUESTION)
    parser.add_argument("--additional-project", type=Path, action="append", default=[])
    parser.add_argument("--additional-query", action="append", default=[])
    parser.add_argument("--timeout-secs", type=int, default=900)
    parser.add_argument("--version-only", action="store_true")
    parser.add_argument(
        "--proof-tier",
        choices=("calibration", "hosted_package", "protected_hardware", "installed_runtime"),
        default="hosted_package",
    )
    parser.add_argument("--plugin-handoff", action="store_true")
    parser.add_argument("--engine-policy", choices=("accelerated", "cpu_explicit"))
    parser.add_argument("--expected-backend")
    parser.add_argument("--qualification-matrix-cell")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--qualification-evidence", type=Path)
    parser.add_argument("--produce-qualification-evidence", action="store_true")
    parser.add_argument("--server-behavior-only", action="store_true")
    parser.add_argument("--ground-only", action="store_true")
    parser.add_argument("--publication-fault-evidence", type=Path)
    parser.add_argument("--retrieval-quality-evidence", type=Path)
    parser.add_argument("--calibration-bundle", type=Path)
    parser.add_argument("--enforce-calibration-freeze-lineage", action="store_true")
    parser.add_argument("--calibration-run-index", type=int)
    parser.add_argument("--calibration-run-output", type=Path)
    parser.add_argument("--assemble-calibration-bundle", action="store_true")
    parser.add_argument("--calibration-run", type=Path, action="append", default=[])
    parser.add_argument("--calibration-bundle-output", type=Path)
    parser.add_argument("--frozen-constant-set-output", type=Path)
    parser.add_argument("--freeze-selected-at")
    parser.add_argument("--calibration-producer-repository")
    parser.add_argument("--calibration-producer-workflow-path")
    parser.add_argument("--calibration-producer-run-id")
    parser.add_argument("--calibration-producer-run-attempt")
    parser.add_argument("--calibration-producer-artifact")
    parser.add_argument("--installed-plugin-attestation", type=Path)
    parser.add_argument("--installed-plugin-data", type=Path)
    parser.add_argument("--prepare-candidate-installed-proof", action="store_true")
    parser.add_argument("--candidate-plugin-root-output", type=Path)
    parser.add_argument("--candidate-plugin-data-output", type=Path)
    parser.add_argument("--installed-plugin-attestation-output", type=Path)
    parser.add_argument("--candidate-producer-repository")
    parser.add_argument("--candidate-producer-workflow-path")
    parser.add_argument("--candidate-producer-run-id")
    parser.add_argument("--candidate-producer-run-attempt")
    parser.add_argument("--candidate-artifact-name")
    parser.add_argument("--measurement-protocol", type=Path, default=MEASUREMENT_PROTOCOL)
    parser.add_argument("--expected-source-sha")
    parser.add_argument("--expected-source-tree")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def _print_json(result: object) -> None:
    print(json.dumps(result, indent=2, sort_keys=True))


def _resolve_optional_paths(args: argparse.Namespace) -> None:
    for field in (
        "qualification_evidence",
        "publication_fault_evidence",
        "retrieval_quality_evidence",
        "calibration_bundle",
        "calibration_run_output",
        "installed_plugin_attestation",
        "installed_plugin_data",
    ):
        value = getattr(args, field)
        if value is not None:
            setattr(args, field, value.resolve())


def _installed_proof_source(args: argparse.Namespace) -> str:
    if args.installed_plugin_attestation is None:
        return "marketplace"
    attestation = json.loads(
        args.installed_plugin_attestation.read_text(encoding="utf-8")
    )
    require(
        isinstance(attestation, dict),
        "installed plugin attestation must be an object",
    )
    source = {
        "candidate_archive": "candidate",
        "codex_marketplace_install": "marketplace",
    }.get(attestation.get("installation_source"))
    require(
        source is not None,
        "installed plugin attestation has an invalid installation source",
    )
    return source


def _prepare_proof_arguments(args: argparse.Namespace) -> None:
    require(
        args.archive and args.checksum_file and args.expected_version,
        "archive, checksum, and expected version are required",
    )
    args.archive = args.archive.resolve()
    args.checksum_file = args.checksum_file.resolve()
    args.out_dir = args.out_dir.resolve()
    _resolve_optional_paths(args)
    require(
        (args.calibration_run_output is None)
        == (args.calibration_run_index is None),
        "--calibration-run-output and --calibration-run-index must be supplied together",
    )
    require(
        args.calibration_run_output is None or args.proof_tier == "calibration",
        "calibration run output is valid only for the calibration proof tier",
    )
    require_candidate_matrix_installation_source(
        args.qualification_matrix_cell,
        _installed_proof_source(args),
    )
    validate_runtime_claim_scope(args)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    require(
        sha256(args.archive)
        == expected_archive_digest(args.checksum_file, args.archive),
        "archive checksum mismatch",
    )


def _verify_package_source(
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


def _load_calibration_bundle(
    args: argparse.Namespace,
    manifest: dict[str, object],
    measurement_contract: dict[str, object],
    *,
    require_frozen: bool,
) -> dict[str, object] | None:
    if not require_frozen:
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


def _claim_scope(args: argparse.Namespace) -> str:
    if args.ground_only:
        return (
            "installed_ground"
            if args.proof_tier == "installed_runtime"
            else "packaged_ground"
        )
    return "server_behavior_only" if args.server_behavior_only else "qualification"


def _package_summary(
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
            "claim_scope": _claim_scope(args),
            "highest_proof_tier": "package_structure",
        },
    }


def _run_runtime_proof(
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


def _installed_runtime_provenance_is_proven(
    args: argparse.Namespace,
    runtime: dict[str, object],
) -> bool:
    return (
        args.proof_tier == "installed_runtime"
        and isinstance(runtime.get("installed_plugin"), dict)
        and isinstance(runtime.get("managed_runtime"), dict)
    )


def _record_runtime_contract(
    args: argparse.Namespace,
    summary: dict[str, object],
    manifest: dict[str, object],
    runtime: dict[str, object],
) -> None:
    package_contract = summary["package_contract"]
    if args.ground_only:
        installed_provenance = _installed_runtime_provenance_is_proven(args, runtime)
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


def _load_evidence(path: Path, label: str) -> dict[str, object]:
    try:
        evidence = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(evidence, dict), f"{label} must be an object")
    return evidence


def _record_retained_qualification(
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
    retained = _load_evidence(args.qualification_evidence, "qualification evidence")
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


def _record_calibration_qualification(
    args: argparse.Namespace,
    summary: dict[str, object],
) -> None:
    if (
        args.qualification_evidence is None
        or not args.qualification_evidence.is_file()
    ):
        return
    calibration = _load_evidence(
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


def _record_qualification_contract(
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
            "release_readiness_claim": False,
            "installed_runtime_provenance_proven": (
                _installed_runtime_provenance_is_proven(args, runtime)
            ),
        }
        summary["package_contract"]["highest_proof_tier"] = "server_behavior"
    elif args.proof_tier == "calibration":
        _record_calibration_qualification(args, summary)
    else:
        _record_retained_qualification(
            args,
            summary,
            manifest,
            runtime,
            measurement_contract,
        )


def _run_archive_proof(args: argparse.Namespace) -> None:
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
        _verify_package_source(args, manifest)
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
        calibration_bundle = _load_calibration_bundle(
            args,
            manifest,
            measurement_contract,
            require_frozen=require_frozen,
        )
        env = isolated_environment(root, args.engine_policy, args.offline)
        summary = _package_summary(
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
            runtime = _run_runtime_proof(
                args,
                cli,
                env,
                root,
                manifest,
                measurement_contract,
            )
            summary["runtime"] = runtime
            _record_runtime_contract(args, summary, manifest, runtime)
            _record_qualification_contract(
                args,
                summary,
                manifest,
                runtime,
                measurement_contract,
            )
        write_json(args.out_dir / "summary.json", summary)


def main() -> int:
    args = parse_args()
    if args.self_test:
        from .self_test import self_test

        self_test()
        return 0
    args.measurement_protocol = args.measurement_protocol.resolve()
    if args.assemble_calibration_bundle:
        _print_json(assemble_calibration_bundle(args))
        return 0
    if args.prepare_candidate_installed_proof:
        _print_json(prepare_candidate_installed_proof(args))
        return 0
    _prepare_proof_arguments(args)
    _run_archive_proof(args)
    print(
        f"packaged CodeStory {args.proof_tier} proof passed: "
        f"{args.out_dir / 'summary.json'}"
    )
    return 0
