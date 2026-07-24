"""Self-tests for packaged-proof CLI orchestration."""

from __future__ import annotations

import argparse
import tempfile
from pathlib import Path

from .cli import (
    _claim_scope,
    _installed_proof_source,
    _record_calibration_qualification,
    _resolve_optional_paths,
)
from .contracts import write_json
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
            publication_fault_evidence=None,
            retrieval_quality_evidence=None,
            calibration_bundle=None,
            calibration_run_output=None,
            installed_plugin_attestation=attestation,
            installed_plugin_data=None,
            proof_tier="installed_runtime",
            ground_only=True,
            server_behavior_only=False,
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
            _claim_scope(args) == "installed_ground",
            "installed ground claim scope changed",
        )

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
        _record_calibration_qualification(args, summary)
        require(
            summary["qualification"]["status"] == "calibration",
            "calibration qualification was not recorded",
        )
