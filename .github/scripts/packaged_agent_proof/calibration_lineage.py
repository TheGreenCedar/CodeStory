"""Exact Git lineage verification for calibration freezes."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

from .contract_primitives import require_nonempty_string
from .foundation import require

# The single file a freeze commit is allowed to write between the tree that was
# calibrated and the tree that is packaged.
CONSTANT_SET_FREEZE_PATH = (
    "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json"
)
# Enforcing the freeze lineage decides the release sequencing: because the
# freeze commit must be the only commit between calibration and the package,
# the version bump cannot follow calibration. Every failure below repeats this
# so a CI reader can act without opening this file.
REQUIRED_RELEASE_ORDERING = (
    "required release ordering is bump-then-calibrate: bump the version first "
    "(node scripts/bump-version.mjs --version <version>), calibrate on the "
    "bumped tree, then land the constant-set freeze commit as the only commit "
    f"between calibration and the packaged release ({CONSTANT_SET_FREEZE_PATH} "
    "is the only file it may write). A calibrate-then-bump ordering fails here: "
    "move the bump ahead of calibration and recalibrate on the bumped tree"
)


def verify_calibration_source_lineage(
    calibration_source: dict,
    frozen_source: dict,
    repository_root: Path,
) -> dict:
    require(
        frozen_source.get("tracked_dirty") is False,
        "frozen package source tree was dirty",
    )
    for label, source in (
        ("calibration", calibration_source),
        ("frozen package", frozen_source),
    ):
        require(
            isinstance(source.get("commit"), str)
            and re.fullmatch(r"[0-9a-f]{40}", source["commit"]) is not None
            and isinstance(source.get("tree"), str)
            and re.fullmatch(r"[0-9a-f]{40}", source["tree"]) is not None,
            f"{label} source identity is not an exact Git commit and tree",
        )
    require(
        calibration_source["commit"] != frozen_source["commit"],
        "frozen package did not add the required constant-set freeze commit",
    )

    def git(*arguments: str) -> str:
        completed = subprocess.run(
            ["git", *arguments],
            cwd=repository_root,
            text=True,
            capture_output=True,
            timeout=30,
        )
        # `require`'s message argument is evaluated before the call, so the
        # detail must not itself be a contract that a *successful* probe can
        # violate. `git diff --name-only` legitimately prints nothing when the
        # two trees are identical, and demanding non-empty output here replaced
        # the actionable drift message below with "Git lineage failure must be a
        # non-empty string".
        require(
            completed.returncode == 0,
            "calibration source-lineage probe failed: "
            + require_nonempty_string(
                completed.stderr.strip()
                or completed.stdout.strip()
                or f"git {' '.join(arguments)} exited {completed.returncode} "
                "without output",
                "Git lineage failure",
            ),
        )
        return completed.stdout.strip()

    require(
        git("rev-parse", "HEAD") == frozen_source["commit"]
        and git("rev-parse", "HEAD^{tree}") == frozen_source["tree"],
        "verification checkout does not match the frozen package source",
    )
    require(
        git("rev-parse", f"{calibration_source['commit']}^{{tree}}")
        == calibration_source["tree"],
        "calibration commit does not resolve to the recorded calibration tree",
    )
    completed = subprocess.run(
        [
            "git",
            "merge-base",
            "--is-ancestor",
            calibration_source["commit"],
            frozen_source["commit"],
        ],
        cwd=repository_root,
        capture_output=True,
        timeout=30,
    )
    require(
        completed.returncode == 0,
        "calibration source "
        f"{calibration_source['commit']} is not an ancestor of the frozen "
        f"package source {frozen_source['commit']}; the packaged tree was not "
        "grown from the calibrated tree, so the frozen constants were never "
        f"measured on what ships. The {REQUIRED_RELEASE_ORDERING}.",
    )
    changed_paths = [
        path
        for path in git(
            "diff",
            "--name-only",
            calibration_source["commit"],
            frozen_source["commit"],
        ).splitlines()
        if path
    ]
    offending_paths = [
        path for path in changed_paths if path != CONSTANT_SET_FREEZE_PATH
    ]
    require(
        changed_paths == [CONSTANT_SET_FREEZE_PATH],
        "post-calibration source drift exceeded the one allowed constant-set "
        "freeze file: "
        + (
            "offending changed paths between calibration "
            f"{calibration_source['commit']} and packaged "
            f"{frozen_source['commit']}: " + ", ".join(offending_paths)
            if offending_paths
            else "the packaged source did not add the required "
            f"{CONSTANT_SET_FREEZE_PATH} freeze commit "
            f"(no path changed between calibration {calibration_source['commit']} "
            f"and packaged {frozen_source['commit']})"
        )
        + f". The {REQUIRED_RELEASE_ORDERING}.",
    )
    return {
        "selection_commit": calibration_source["commit"],
        "frozen_commit": frozen_source["commit"],
        "allowed_changed_paths": changed_paths,
    }
