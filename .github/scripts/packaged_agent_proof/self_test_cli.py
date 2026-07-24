"""Self-tests for packaged-proof CLI orchestration."""

from __future__ import annotations

import argparse
import tempfile
from pathlib import Path

from .archive_proof import claim_scope, load_calibration_bundle, requires_calibration_bundle
from .cli import _installed_proof_source, _resolve_optional_paths
from .contract_primitives import write_json
from .foundation import ProofFailure, require
from .qualification_recording import record_calibration_qualification


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
            calibration_run_output=None,
            installed_plugin_attestation=attestation,
            installed_plugin_data=None,
            proof_tier="installed_runtime",
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
            _installed_proof_source(args) == "candidate",
            "candidate installation source was not retained",
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

        write_json(attestation, {"installation_source": "unsupported"})
        try:
            _installed_proof_source(args)
        except ProofFailure:
            pass
        else:
            raise ProofFailure("unsupported installation source was accepted")

        calibration = root / "calibration.json"
        write_json(
            calibration,
            {
                "schema_version": 1,
                "status": "calibration",
                "tier": "calibration",
            },
        )
        args.qualification_evidence = calibration
        summary: dict[str, object] = {}
        record_calibration_qualification(args, summary)
        require(
            summary["qualification"]["status"] == "calibration",
            "calibration qualification was not recorded",
        )
