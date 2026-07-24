"""Cli for packaged CodeStory proof."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from .archive import expected_archive_digest
from .archive_proof import run_archive_proof
from .calibration import assemble_calibration_bundle
from .contracts import (
    sha256,
    validate_runtime_claim_scope,
)
from .foundation import DEFAULT_QUERY, DEFAULT_QUESTION, MEASUREMENT_PROTOCOL, require
from .installation import (
    prepare_candidate_installed_proof,
)
from .qualification import (
    require_candidate_matrix_installation_source,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--checksum-file", type=Path)
    parser.add_argument("--expected-version")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--plugin-root", type=Path)
    parser.add_argument(
        "--out-dir", type=Path, default=Path("target/packaged-agent-proof")
    )
    parser.add_argument("--query", default=DEFAULT_QUERY)
    parser.add_argument("--question", default=DEFAULT_QUESTION)
    parser.add_argument("--additional-project", type=Path, action="append", default=[])
    parser.add_argument("--additional-query", action="append", default=[])
    parser.add_argument("--timeout-secs", type=int, default=900)
    parser.add_argument("--version-only", action="store_true")
    parser.add_argument(
        "--proof-tier",
        choices=(
            "calibration",
            "hosted_package",
            "protected_hardware",
            "installed_runtime",
        ),
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
    parser.add_argument(
        "--measurement-protocol", type=Path, default=MEASUREMENT_PROTOCOL
    )
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
        (args.calibration_run_output is None) == (args.calibration_run_index is None),
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
    run_archive_proof(args)
    print(
        f"packaged CodeStory {args.proof_tier} proof passed: "
        f"{args.out_dir / 'summary.json'}"
    )
    return 0
