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
from packaged_agent_proof.frozen_acceptance import (
    MEASUREMENT_PROTOCOL_PATH,
    resolve_frozen_acceptance_identity,
    verify_frozen_candidate_acceptance,
)
from packaged_agent_proof.foundation import ProofFailure
from packaged_agent_proof.measurement_protocol import (
    load_server_measurement_contract,
)


def _append_github_outputs(path: Path, values: dict[str, str]) -> None:
    with path.open("a", encoding="utf-8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Verify that the exact release head differs from the calibrated "
            "source only by the frozen constant-set file."
        )
    )
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--expected-sha", required=True)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--calibration-bundle", type=Path)
    parser.add_argument("--artifact-constant-set", type=Path)
    parser.add_argument("--expected-producer-run-id")
    parser.add_argument("--expected-producer-run-attempt")
    parser.add_argument("--expected-producer-artifact")
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
    frozen_inputs = (
        arguments.calibration_bundle,
        arguments.artifact_constant_set,
        arguments.expected_producer_run_id,
        arguments.expected_producer_run_attempt,
        arguments.expected_producer_artifact,
    )
    if any(value is not None for value in frozen_inputs):
        if arguments.allow_promotion_commit or not all(
            value is not None for value in frozen_inputs
        ):
            raise ProofFailure(
                "frozen acceptance requires the complete bundle, artifact constant "
                "set, producer run, attempt, and artifact identity without a promotion "
                "exception"
            )
        result = verify_frozen_candidate_acceptance(
            repository_root,
            arguments.expected_sha,
            calibration_bundle=arguments.calibration_bundle,
            artifact_constant_set=arguments.artifact_constant_set,
            producer_run_id=arguments.expected_producer_run_id,
            producer_run_attempt=arguments.expected_producer_run_attempt,
            producer_artifact=arguments.expected_producer_artifact,
        )
        identity = result["identity"]
    else:
        lineage = verify_release_head_calibration_lineage(
            repository_root,
            arguments.expected_sha,
            allow_promotion_commit=arguments.allow_promotion_commit,
        )
        if arguments.github_output is None:
            result = lineage
            identity = None
        else:
            contract = load_server_measurement_contract(
                repository_root / MEASUREMENT_PROTOCOL_PATH
            )
            identity = resolve_frozen_acceptance_identity(contract)
            result = {"lineage": lineage, "identity": identity}
    if arguments.github_output is not None:
        if identity is None:
            raise ProofFailure("GitHub output requires frozen acceptance identity")
        _append_github_outputs(arguments.github_output, identity)
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
