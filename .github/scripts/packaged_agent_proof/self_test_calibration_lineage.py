"""Self-tests for the calibration-to-package source lineage guard.

``verify_calibration_source_lineage`` is the strongest binding the calibration
freeze has: the calibrated commit must be an ancestor of the packaged commit and
the freeze file must be the only path that differs. Until the guard was turned
on in CI nothing exercised it -- every caller in the tree passed
``enforce_source_lineage=False`` -- so it could have been deleted, inverted, or
quietly weakened without one test objecting. These tests build real throwaway Git
histories and drive the guard directly, in both directions, including the
calibrate-then-bump ordering that the enabled guard now rejects.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
from collections.abc import Iterable
from pathlib import Path

from . import archive_proof
from .calibration_lineage import (
    CONSTANT_SET_FREEZE_PATH,
    verify_calibration_source_lineage,
)
from .foundation import REPOSITORY_ROOT, ProofFailure, require

CARGO_MANIFEST_PATH = "crates/codestory-cli/Cargo.toml"
_GIT_ENVIRONMENT = {
    "GIT_AUTHOR_NAME": "CodeStory Proof",
    "GIT_AUTHOR_EMAIL": "proof@codestory.invalid",
    "GIT_COMMITTER_NAME": "CodeStory Proof",
    "GIT_COMMITTER_EMAIL": "proof@codestory.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00+00:00",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00+00:00",
    # A developer's global signing or hook configuration must not decide whether
    # this fixture repository can commit.
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_SYSTEM": os.devnull,
}


def _git(root: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", "-c", "commit.gpgsign=false", *arguments],
        cwd=root,
        text=True,
        capture_output=True,
        timeout=60,
        env={**os.environ, **_GIT_ENVIRONMENT},
    )
    require(
        completed.returncode == 0,
        f"calibration lineage self-test git {' '.join(arguments)} failed: "
        + (completed.stderr.strip() or completed.stdout.strip() or "no output"),
    )
    return completed.stdout.strip()


def _write(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _constant_set(status: str) -> str:
    return json.dumps({"status": status}, indent=2) + "\n"


def _cargo_manifest(version: str) -> str:
    return f'[package]\nname = "codestory-cli"\nversion = "{version}"\n'


def _commit(root: Path, message: str, *, allow_empty: bool = False) -> dict:
    _git(root, "add", "-A")
    arguments = ["commit", "--no-verify", "-q", "-m", message]
    if allow_empty:
        arguments.insert(1, "--allow-empty")
    _git(root, *arguments)
    return {
        "commit": _git(root, "rev-parse", "HEAD"),
        "tree": _git(root, "rev-parse", "HEAD^{tree}"),
        "tracked_dirty": False,
    }


def _reject(
    label: str,
    fragments: Iterable[str],
    calibration_source: dict,
    frozen_source: dict,
    root: Path,
) -> None:
    try:
        verify_calibration_source_lineage(calibration_source, frozen_source, root)
    except ProofFailure as failure:
        message = str(failure)
        for fragment in fragments:
            require(
                fragment in message,
                f"{label} rejection message omitted {fragment!r}: {message}",
            )
    else:
        raise ProofFailure(
            f"{label} was accepted by the calibration source-lineage guard"
        )


def _build_calibration_history(root: Path) -> dict:
    _git(root, "-c", "init.defaultBranch=main", "init", "-q")
    _write(root, "README.md", "calibration lineage fixture\n")
    _write(root, CARGO_MANIFEST_PATH, _cargo_manifest("0.16.1"))
    _write(root, CONSTANT_SET_FREEZE_PATH, _constant_set("unfrozen"))
    return _commit(root, "calibrated tree")


def _accepts_the_single_freeze_commit(root: Path, calibration: dict) -> dict:
    _write(root, CONSTANT_SET_FREEZE_PATH, _constant_set("frozen"))
    frozen = _commit(root, "freeze the constant set")
    lineage = verify_calibration_source_lineage(calibration, frozen, root)
    require(
        lineage
        == {
            "selection_commit": calibration["commit"],
            "frozen_commit": frozen["commit"],
            "freeze_commit": frozen["commit"],
            "promotion_commit": None,
            "allowed_changed_paths": [CONSTANT_SET_FREEZE_PATH],
        },
        "the one allowed constant-set freeze commit was not accepted intact",
    )
    return frozen


def _rejects_commit_after_freeze(
    root: Path,
    calibration: dict,
    frozen: dict,
) -> None:
    later = _commit(root, "later empty commit", allow_empty=True)
    _reject(
        "a later tree-preserving commit",
        ["direct single-parent child", "later commit revokes acceptance"],
        calibration,
        later,
        root,
    )
    accepted_promotion = verify_calibration_source_lineage(
        calibration,
        later,
        root,
        allow_promotion_commit=True,
    )
    require(
        accepted_promotion["freeze_commit"] == frozen["commit"]
        and accepted_promotion["promotion_commit"] == later["commit"],
        "the explicit tree-preserving promotion exception lost its exact commits",
    )
    _git(root, "reset", "-q", "--hard", frozen["commit"])


def _rejects_identity_and_checkout_drift(
    root: Path,
    calibration: dict,
    frozen: dict,
) -> None:
    _reject(
        "a dirty packaged source tree",
        ["frozen package source tree was dirty"],
        calibration,
        {**frozen, "tracked_dirty": True},
        root,
    )
    _reject(
        "an inexact calibration source identity",
        ["calibration source identity is not an exact Git commit and tree"],
        {**calibration, "commit": "not-a-commit"},
        frozen,
        root,
    )
    _reject(
        "a package that added no freeze commit at all",
        ["frozen package did not add the required constant-set freeze commit"],
        frozen,
        frozen,
        root,
    )
    _reject(
        "a packaged source the verification checkout does not hold",
        ["verification checkout does not match the frozen package source"],
        calibration,
        {**frozen, "tree": calibration["tree"]},
        root,
    )
    _reject(
        "a calibration tree that its own commit does not resolve to",
        ["calibration commit does not resolve to the recorded calibration tree"],
        {**calibration, "tree": frozen["tree"]},
        frozen,
        root,
    )


def _rejects_calibrate_then_bump(root: Path, calibration: dict) -> None:
    """The sequencing decision the enabled guard makes for the release runbook.

    Calibrating first and bumping the version afterwards puts a second commit
    between the calibrated tree and the packaged tree, so the frozen constants
    were measured on a tree that is not the one shipping. The guard must reject
    it, and the message must name the offending path and the required ordering
    so a release operator can act from the CI log alone.
    """
    _git(root, "checkout", "-q", "-b", "calibrate-then-bump", calibration["commit"])
    _write(root, CARGO_MANIFEST_PATH, _cargo_manifest("0.16.2"))
    _commit(root, "bump the version after calibration")
    _write(root, CONSTANT_SET_FREEZE_PATH, _constant_set("frozen"))
    bumped_after_calibration = _commit(root, "freeze the constant set")
    _reject(
        "a version bump landing after calibration",
        [
            "post-calibration source drift exceeded the one allowed constant-set "
            "freeze file",
            CARGO_MANIFEST_PATH,
            "bump-then-calibrate",
            "recalibrate on the bumped tree",
        ],
        calibration,
        bumped_after_calibration,
        root,
    )
    require(
        CONSTANT_SET_FREEZE_PATH
        in _git(
            root,
            "diff",
            "--name-only",
            calibration["commit"],
            bumped_after_calibration["commit"],
        ),
        "the calibrate-then-bump fixture did not also land the freeze file",
    )


def _rejects_missing_freeze_and_unrelated_history(
    root: Path,
    frozen: dict,
) -> None:
    _git(root, "checkout", "-q", "main")
    empty_freeze = _commit(root, "package without freezing anything", allow_empty=True)
    _reject(
        "a packaged commit that changed no path at all",
        [
            "post-calibration source drift exceeded the one allowed constant-set "
            "freeze file",
            "did not add the required",
            CONSTANT_SET_FREEZE_PATH,
            "bump-then-calibrate",
        ],
        frozen,
        empty_freeze,
        root,
    )
    _git(root, "reset", "-q", "--hard", frozen["commit"])
    _git(root, "checkout", "-q", "--orphan", "unrelated")
    _write(root, "unrelated.txt", "measured somewhere else entirely\n")
    unrelated = _commit(root, "calibrate on unrelated history")
    _git(root, "checkout", "-q", "main")
    require(
        _git(root, "rev-parse", "HEAD") == frozen["commit"],
        "the lineage fixture lost its frozen checkout",
    )
    _reject(
        "calibration measured on unrelated history",
        [
            "is not an ancestor of the frozen package source",
            unrelated["commit"],
            frozen["commit"],
            "bump-then-calibrate",
        ],
        unrelated,
        frozen,
        root,
    )


_PROBE_MANIFEST = {
    "source": {"commit": "a" * 40, "tree": "b" * 40, "tracked_dirty": False},
    "asset_target": "linux-x64",
    "release_version": "0.0.0",
}
_PROBE_CONTRACT = {
    "constant_set": {},
    "protocol_sha256": "protocol",
    "constant_set_sha256": "constants",
    "measurement_protocol_sha256": "measurement",
}


def _lineage_probe_arguments(**overrides: object) -> argparse.Namespace:
    """The exact argument shape the packaged proof lineage step dispatches."""
    values: dict[str, object] = {
        "archive": Path("archive.tar.gz"),
        "checksum_file": Path("SHA256SUMS.txt"),
        "expected_version": "0.0.0",
        "expected_source_sha": "a" * 40,
        "expected_source_tree": "b" * 40,
        "measurement_protocol": Path("measurement-protocol.json"),
        "out_dir": Path("target/packaged-calibration-lineage"),
        "project": None,
        "engine_policy": None,
        "offline": False,
        "version_only": True,
        "proof_tier": "hosted_package",
        "server_behavior_only": False,
        "ground_only": False,
        "produce_qualification_evidence": False,
        "qualification_evidence": None,
        "enforce_calibration_freeze_lineage": True,
        "calibration_bundle": Path("calibration-bundle.json"),
        "calibration_producer_run_id": "1234567890",
        "calibration_producer_artifact": "embedding-calibration-bundle-" + "c" * 40,
        "timeout_secs": 1800,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def _run_lineage_probe(args: argparse.Namespace) -> dict:
    """Run the real proof pipeline with only archive and CLI layers stubbed.

    Everything the lineage step depends on -- the frozen-contract requirement,
    the calibration bundle load, and the enforcement flag reaching
    ``verify_calibration_bundle`` -- stays real. Unpacking a synthetic archive
    and executing a packaged binary do not, because neither decides whether the
    guard runs.
    """
    observed: dict = {}
    originals: dict = {}

    def stub(name: str, value: object) -> None:
        originals[name] = getattr(archive_proof, name)
        setattr(archive_proof, name, value)

    def record_contracts(manifest, protocol, *, require_frozen):
        observed["require_frozen"] = require_frozen
        return _PROBE_CONTRACT

    def record_bundle(path, contract, **kwargs):
        observed["bundle_verification"] = {"path": path, **kwargs}
        return {"freeze_digest": "digest", "source_lineage": {"verified": True}}

    stub("unpack_archive", lambda archive, destination: None)
    stub("find_cli", lambda root: Path("codestory-cli"))
    stub("load_native_manifest", lambda root, cli, version: _PROBE_MANIFEST)
    stub("verify_package_source", lambda args, manifest: None)
    stub("verify_package_server_contracts", record_contracts)
    stub("verify_calibration_bundle", record_bundle)
    stub("isolated_environment", lambda root, policy, offline: {})
    stub("package_summary", lambda *call, **keywords: {"package_contract": {}})
    stub("write_json", lambda path, payload: None)
    try:
        archive_proof.run_archive_proof(args)
        observed["outcome"] = "accepted"
    except ProofFailure as failure:
        observed["outcome"] = str(failure)
    finally:
        for name, value in originals.items():
            setattr(archive_proof, name, value)
    return observed


def _version_only_invocation_enforces_the_lineage() -> None:
    """Pin the CLI shape the reachable packaged-proof lineage step dispatches.

    The full frozen ``hosted_package`` qualification cannot run without the
    optional exact-head release-evidence packet, which the frozen-candidate
    coordinator is forbidden to carry. ``--version-only`` stops before the
    runtime proof but still loads and verifies the authenticated bundle, so it
    is the shape that can actually enforce the freeze lineage in CI. If that
    stops being true this test fails rather than the guard silently going dark.
    """
    enforced = _run_lineage_probe(_lineage_probe_arguments())
    require(
        enforced["outcome"] == "accepted",
        f"the version-only lineage invocation was rejected: {enforced['outcome']}",
    )
    require(
        enforced.get("require_frozen") is True,
        "the version-only lineage invocation stopped requiring a frozen contract",
    )
    verification = enforced.get("bundle_verification")
    require(
        isinstance(verification, dict)
        and verification.get("enforce_source_lineage") is True
        and verification.get("frozen_source") == _PROBE_MANIFEST["source"]
        and verification.get("repository_root") == REPOSITORY_ROOT
        and verification.get("expected_producer_run_id") == "1234567890"
        and verification.get("expected_producer_artifact")
        == "embedding-calibration-bundle-" + "c" * 40,
        "the version-only lineage invocation did not enforce the source lineage "
        f"against the packaged source: {verification}",
    )
    # The flag is load-bearing rather than decorative here: dropping it does not
    # quietly downgrade this step to an unchecked package smoke, it makes the
    # bundle arguments illegal and the step fails closed.
    unenforced = _run_lineage_probe(
        _lineage_probe_arguments(enforce_calibration_freeze_lineage=False)
    )
    require(
        unenforced["outcome"] == "qualification proof rejects calibration inputs",
        "a version-only proof accepted calibration inputs without enforcing the "
        f"freeze lineage: {unenforced['outcome']}",
    )
    require(
        "bundle_verification" not in unenforced,
        "an unenforced version-only proof still verified the calibration bundle",
    )


def run_calibration_lineage_self_tests() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw) / "calibration-lineage"
        root.mkdir(parents=True)
        calibration = _build_calibration_history(root)
        frozen = _accepts_the_single_freeze_commit(root, calibration)
        _rejects_commit_after_freeze(root, calibration, frozen)
        _rejects_identity_and_checkout_drift(root, calibration, frozen)
        _rejects_calibrate_then_bump(root, calibration)
        _rejects_missing_freeze_and_unrelated_history(root, frozen)
    _version_only_invocation_enforces_the_lineage()
