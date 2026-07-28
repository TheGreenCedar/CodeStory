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

import json
import os
import subprocess
import tempfile
from collections.abc import Iterable
from pathlib import Path

from .calibration_lineage import (
    CONSTANT_SET_FREEZE_PATH,
    verify_calibration_source_lineage,
)
from .foundation import ProofFailure, require

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
            "allowed_changed_paths": [CONSTANT_SET_FREEZE_PATH],
        },
        "the one allowed constant-set freeze commit was not accepted intact",
    )
    return frozen


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


def run_calibration_lineage_self_tests() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw) / "calibration-lineage"
        root.mkdir(parents=True)
        calibration = _build_calibration_history(root)
        frozen = _accepts_the_single_freeze_commit(root, calibration)
        _rejects_identity_and_checkout_drift(root, calibration, frozen)
        _rejects_calibrate_then_bump(root, calibration)
        _rejects_missing_freeze_and_unrelated_history(root, frozen)
