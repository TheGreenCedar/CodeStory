"""Self-tests for packaged-proof CLI orchestration."""

from __future__ import annotations

import argparse
import tempfile
from pathlib import Path

from .archive_proof import claim_scope, load_calibration_bundle, requires_calibration_bundle
from .cli import _resolve_optional_paths, _validate_calibration_mode
from .contract_primitives import write_json
from .foundation import ProofFailure, require


def run_cli_self_tests() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        attestation = root / "candidate-attestation.json"
        write_json(
            attestation,
            {"installation_source": "candidate_archive"},
        )
        args = argparse.Namespace(
            qualification_evidence=None,
            qualification_driver=None,
            publication_fault_evidence=None,
            retrieval_quality_evidence=None,
            calibration_bundle=None,
            collect_constant_calibration=False,
            constant_calibration_output_dir=None,
            installed_plugin_attestation=attestation,
            installed_plugin_data=None,
            out_dir=root / "proof",
            proof_tier="installed_runtime",
            engine_policy="accelerated",
            expected_backend="metal",
            offline=True,
            project=None,
            plugin_root=None,
            plugin_handoff=False,
            additional_project=[],
            additional_query=[],
            produce_qualification_evidence=False,
            ground_only=True,
            server_behavior_only=False,
            version_only=False,
            enforce_calibration_freeze_lineage=False,
            calibration_producer_run_id=None,
            calibration_producer_artifact=None,
        )
        _resolve_optional_paths(args)
        require(
            args.installed_plugin_attestation == attestation.resolve(),
            "CLI optional path resolution changed",
        )
        require(
            claim_scope(args) == "installed_ground",
            "installed ground claim scope changed",
        )
        require(
            not requires_calibration_bundle(args),
            "ground-only proof unexpectedly requires a calibration bundle",
        )
        require(
            load_calibration_bundle(args, {}, {}, required=False) is None,
            "ground-only proof unexpectedly loaded a calibration bundle",
        )
        args.calibration_bundle = attestation
        try:
            load_calibration_bundle(args, {}, {}, required=False)
        except ProofFailure:
            pass
        else:
            raise ProofFailure("ground-only proof accepted a calibration bundle")
        args.calibration_bundle = None

        args.ground_only = False
        args.server_behavior_only = True
        require(
            not requires_calibration_bundle(args),
            "server-behavior-only proof unexpectedly requires a calibration bundle",
        )

        args.server_behavior_only = False
        require(
            requires_calibration_bundle(args),
            "qualification proof no longer requires a calibration bundle",
        )
        try:
            load_calibration_bundle(args, {}, {}, required=True)
        except ProofFailure:
            pass
        else:
            raise ProofFailure("qualification proof accepted a missing calibration bundle")

        args.server_behavior_only = True
        args.enforce_calibration_freeze_lineage = True
        require(
            requires_calibration_bundle(args),
            "freeze-lineage proof no longer requires a calibration bundle",
        )
        args.enforce_calibration_freeze_lineage = False
        args.server_behavior_only = False

        calibration_args = argparse.Namespace(**vars(args))
        calibration_args.proof_tier = "calibration"
        calibration_args.ground_only = False
        calibration_args.server_behavior_only = False
        try:
            _validate_calibration_mode(calibration_args)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                "calibration tier reached the full qualification path without its collector"
            )
        calibration_args.collect_constant_calibration = True
        calibration_args.constant_calibration_output_dir = root / "constant-runs"
        calibration_args.qualification_driver = attestation
        _validate_calibration_mode(calibration_args)
        calibration_args.produce_qualification_evidence = True
        try:
            _validate_calibration_mode(calibration_args)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                "constant calibration accepted a full qualification producer"
            )
        calibration_args.produce_qualification_evidence = False
        calibration_args.plugin_handoff = True
        try:
            _validate_calibration_mode(calibration_args)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                "constant calibration accepted packaged plugin handoff"
            )
        calibration_args.plugin_handoff = False
        calibration_args.plugin_root = attestation
        try:
            _validate_calibration_mode(calibration_args)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                "constant calibration accepted a packaged plugin root"
            )
        calibration_args.plugin_root = None
        calibration_args.engine_policy = "cpu_explicit"
        calibration_args.expected_backend = "cpu"
        try:
            _validate_calibration_mode(calibration_args)
        except ProofFailure:
            pass
        else:
            raise ProofFailure("constant calibration accepted CPU execution")
