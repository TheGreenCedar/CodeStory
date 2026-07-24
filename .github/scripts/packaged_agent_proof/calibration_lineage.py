"""Exact Git lineage verification for calibration freezes."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

from .contracts import require_nonempty_string
from .foundation import require


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
        require(
            completed.returncode == 0,
            "calibration source-lineage probe failed: "
            + require_nonempty_string(
                completed.stderr.strip() or completed.stdout.strip(),
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
        "calibration source is not an ancestor of the frozen package source",
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
    require(
        changed_paths == ["docs/testing/per-user-embedding-server-constant-set.json"],
        "post-calibration source drift exceeded the one allowed constant-set freeze file",
    )
    return {
        "selection_commit": calibration_source["commit"],
        "frozen_commit": frozen_source["commit"],
        "allowed_changed_paths": changed_paths,
    }
