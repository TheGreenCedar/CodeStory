"""Self-tests proving failed proof runs preserve qualification evidence."""

from __future__ import annotations

import argparse
import json
import os
import tempfile
from pathlib import Path

from . import archive_proof
from .failure_evidence import (
    FAILURE_EVIDENCE_DIRECTORY_NAME,
    REDACTION_MARKER,
    preserve_failure_evidence,
    register_failure_evidence_secret,
    reset_failure_evidence_secrets,
)
from .foundation import ProofFailure, require
from .self_test_full_stack_types import FullStackFixture

_UNIT_SECRET = "e" * 64
_SCRIPTED_SECRET = "f" * 64
_SCRIPTED_FAILURE = "scripted qualification driver failure"


def _read(path: Path) -> bytes:
    require(
        path.is_file() and not path.is_symlink(),
        f"preserved evidence is missing: {path}",
    )
    return path.read_bytes()


def _require_redacted(payload: bytes, secret: str, context: str) -> None:
    require(
        secret.encode("utf-8") not in payload,
        f"{context} leaked a registered qualification secret",
    )
    require(
        REDACTION_MARKER.encode("utf-8") in payload,
        f"{context} lost its redaction marker",
    )


def _preservation_unit_tests() -> None:
    with tempfile.TemporaryDirectory() as raw:
        base = Path(raw)
        root = base / "root"
        out_dir = base / "out"
        artifact_root = root / "qualification-suite" / "artifacts"
        artifact_root.mkdir(parents=True)
        out_dir.mkdir()
        register_failure_evidence_secret(_UNIT_SECRET)
        (artifact_root / "request.json").write_text(
            json.dumps({"schema_version": 1, "qualification_nonce": _UNIT_SECRET}),
            encoding="utf-8",
        )
        (root / "qualification-suite" / "worker.log").write_text(
            f"worker crashed after handshake nonce={_UNIT_SECRET}\n",
            encoding="utf-8",
        )
        (root / "qualification").mkdir()
        (root / "qualification" / "gate.json").write_text(
            '{"state": "open"}\n', encoding="utf-8"
        )
        symlink_supported = True
        try:
            os.symlink(base, root / "qualification-suite" / "escape")
        except (OSError, NotImplementedError):
            symlink_supported = False
        error = ProofFailure(f"unit failure carrying {_UNIT_SECRET}")
        preserve_failure_evidence(root, out_dir, error)
        evidence = out_dir / FAILURE_EVIDENCE_DIRECTORY_NAME
        _require_redacted(
            _read(evidence / "qualification-suite" / "artifacts" / "request.json"),
            _UNIT_SECRET,
            "preserved request.json",
        )
        _require_redacted(
            _read(evidence / "qualification-suite" / "worker.log"),
            _UNIT_SECRET,
            "preserved worker log",
        )
        require(
            _read(evidence / "qualification" / "gate.json")
            == b'{"state": "open"}\n',
            "preserved server-identity qualification evidence changed",
        )
        if symlink_supported:
            require(
                not (evidence / "qualification-suite" / "escape").exists(),
                "preservation followed a symlink out of the evidence root",
            )
        manifest = json.loads(_read(evidence / "failure.json"))
        require(
            "unit failure" in manifest["error"]
            and _UNIT_SECRET not in manifest["error"],
            "failure manifest lost or leaked the primary error",
        )
        require(
            manifest["preserved_directories"]
            == ["qualification-suite", "qualification"],
            "failure manifest changed its preserved directory contract",
        )

    with tempfile.TemporaryDirectory() as raw:
        base = Path(raw)
        root = base / "root"
        out_dir = base / "out"
        root.mkdir()
        out_dir.mkdir()
        preserve_failure_evidence(root, out_dir, ProofFailure("early failure"))
        require(
            not any(out_dir.iterdir()),
            "preservation created artifacts before qualification evidence existed",
        )


def _scripted_proof_arguments(fixture: FullStackFixture, out_dir: Path) -> argparse.Namespace:
    return argparse.Namespace(
        archive=fixture.root / "artifact.zip",
        expected_version="0.0.0",
        expected_source_sha=None,
        expected_source_tree=None,
        measurement_protocol=fixture.measurement_protocol,
        out_dir=out_dir,
        project=fixture.root,
        engine_policy="cpu_explicit",
        offline=True,
        version_only=False,
        proof_tier="calibration",
        server_behavior_only=False,
        ground_only=False,
        enforce_calibration_freeze_lineage=False,
        calibration_bundle=None,
        calibration_producer_run_id=None,
        calibration_producer_artifact=None,
        timeout_secs=60,
    )


def _scripted_driver_failure_tests(fixture: FullStackFixture) -> None:
    observed: dict[str, Path] = {}

    def failing_runtime_proof(args, cli, env, root, manifest, measurement_contract):
        observed["root"] = root
        register_failure_evidence_secret(_SCRIPTED_SECRET)
        artifact_root = root / "qualification-suite" / "artifacts"
        artifact_root.mkdir(parents=True)
        (artifact_root / "request.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "qualification_nonce": _SCRIPTED_SECRET,
                    "output_directory": str(artifact_root),
                }
            ),
            encoding="utf-8",
        )
        (artifact_root / "measurement-samples.partial.jsonl").write_text(
            '{"sample_index": 0}\n', encoding="utf-8"
        )
        raise ProofFailure(_SCRIPTED_FAILURE)

    def stub_package_summary(*call_args, **call_kwargs):
        return {"package_contract": {}}

    original_runtime_proof = archive_proof.run_runtime_proof
    original_package_summary = archive_proof.package_summary
    archive_proof.run_runtime_proof = failing_runtime_proof
    archive_proof.package_summary = stub_package_summary
    try:
        with tempfile.TemporaryDirectory() as raw:
            out_dir = Path(raw) / "packaged-agent-proof"
            out_dir.mkdir(parents=True)
            args = _scripted_proof_arguments(fixture, out_dir)
            try:
                archive_proof.run_archive_proof(args)
            except ProofFailure as error:
                require(
                    _SCRIPTED_FAILURE in str(error),
                    "scripted driver failure changed its primary error",
                )
            else:
                raise ProofFailure("scripted driver failure did not propagate")
            root = observed.get("root")
            require(
                root is not None and not root.exists(),
                "temporary package root survived the failed proof",
            )
            evidence = out_dir / FAILURE_EVIDENCE_DIRECTORY_NAME
            _require_redacted(
                _read(
                    evidence / "qualification-suite" / "artifacts" / "request.json"
                ),
                _SCRIPTED_SECRET,
                "upload-root request.json",
            )
            require(
                _read(
                    evidence
                    / "qualification-suite"
                    / "artifacts"
                    / "measurement-samples.partial.jsonl"
                )
                == b'{"sample_index": 0}\n',
                "upload-root partial measurement samples changed",
            )
            manifest = json.loads(_read(evidence / "failure.json"))
            require(
                _SCRIPTED_FAILURE in manifest["error"],
                "upload-root failure manifest lost the driver failure",
            )
            require(
                not (out_dir / "summary.json").exists(),
                "failed proof still wrote a passing summary",
            )
    finally:
        archive_proof.run_runtime_proof = original_runtime_proof
        archive_proof.package_summary = original_package_summary


def run_failure_evidence_self_tests(fixture: FullStackFixture) -> None:
    try:
        _preservation_unit_tests()
        _scripted_driver_failure_tests(fixture)
    finally:
        reset_failure_evidence_secrets()
