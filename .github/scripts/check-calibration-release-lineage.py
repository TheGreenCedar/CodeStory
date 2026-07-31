#!/usr/bin/env python3
"""Fail a release whose source tree was not the calibrated frozen tree."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

from packaged_agent_proof.calibration_lineage import (
    verify_release_head_calibration_lineage,
)
from packaged_agent_proof.foundation import ProofFailure


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Verify that the exact release head differs from the calibrated "
            "source only by the frozen constant-set file."
        )
    )
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--expected-sha", required=True)
    parser.add_argument(
        "--allow-promotion-commit",
        action="store_true",
        help=(
            "Permit one tree-preserving main promotion commit whose release "
            "parent is the direct constant-freeze child."
        ),
    )
    arguments = parser.parse_args()

    repository_root = arguments.repo.resolve(strict=True)
    result = verify_release_head_calibration_lineage(
        repository_root,
        arguments.expected_sha,
        allow_promotion_commit=arguments.allow_promotion_commit,
    )
    print(json.dumps({"status": "passed", **result}, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (
        ProofFailure,
        subprocess.TimeoutExpired,
        OSError,
        json.JSONDecodeError,
    ) as exc:
        print(f"calibration release lineage failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
