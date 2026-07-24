#!/usr/bin/env python3
"""Stable command-line wrapper for packaged CodeStory proof."""

import json
import subprocess
import sys

from packaged_agent_proof.cli import main
from packaged_agent_proof.contracts import ProofFailure


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ProofFailure, subprocess.TimeoutExpired, OSError, json.JSONDecodeError) as exc:
        print(f"packaged CodeStory proof failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
