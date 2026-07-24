"""Qualification evidence production workflow."""

from __future__ import annotations

import argparse
from pathlib import Path

from .qualification_metrics import collect_qualification_measurements
from .qualification_output import write_qualification_outputs
from .qualification_production import (
    collect_qualification_external_evidence,
    collect_qualification_scenarios,
    prepare_qualification_producer,
    run_qualification_producer,
)


def produce_qualification_evidence(
    args: argparse.Namespace,
    qualification_cli: Path,
    env: dict[str, str],
    root: Path,
    runtime: dict,
    manifest: dict,
    archive_sha256: str,
    measurement_contract: dict,
    server_cleanup_control: dict,
) -> dict:
    context = prepare_qualification_producer(
        args,
        qualification_cli,
        env,
        root,
        runtime,
        manifest,
        archive_sha256,
        measurement_contract,
        server_cleanup_control,
    )
    external = collect_qualification_external_evidence(context)
    runner = run_qualification_producer(context)
    scenarios = collect_qualification_scenarios(context, runner, external)
    measurements = collect_qualification_measurements(
        context,
        runner,
        external,
    )
    return write_qualification_outputs(
        context,
        runner,
        scenarios,
        measurements,
    )
