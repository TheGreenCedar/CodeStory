#!/usr/bin/env python3
"""Emit the deterministic accept/reject artifact receipt for issue #1228."""

from __future__ import annotations

import argparse
from pathlib import Path

from packaged_agent_proof.leaf_1228_receipt import build_issue_1228_receipt, write_receipt


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--candidate-tree", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-run-json", required=True, type=Path)
    parser.add_argument("--source-jobs-json", required=True, type=Path)
    parser.add_argument("--source-artifacts-json", required=True, type=Path)
    parser.add_argument("--source-proof-artifact-container", required=True, type=Path)
    parser.add_argument("--candidate-run-json", required=True, type=Path)
    parser.add_argument("--candidate-jobs-json", required=True, type=Path)
    parser.add_argument("--candidate-artifacts-json", required=True, type=Path)
    parser.add_argument("--package-artifact-container", required=True, type=Path)
    parser.add_argument("--archive-record-artifact-container", required=True, type=Path)
    parser.add_argument("--version-proof-artifact-container", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    arguments = vars(parser.parse_args())
    output = arguments.pop("out")
    receipt = build_issue_1228_receipt(**arguments)
    write_receipt(output, receipt)
    print(output)
    return 0 if receipt["decision"] == "accept" else 1


if __name__ == "__main__":
    raise SystemExit(main())
