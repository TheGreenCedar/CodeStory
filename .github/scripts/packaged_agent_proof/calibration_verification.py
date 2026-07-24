"""Verification entry point for retained calibration bundles."""

from __future__ import annotations

from pathlib import Path

from .calibration_freeze import _calibration_freeze
from .calibration_metrics import (
    _selected_calibration_constants,
    _selected_calibration_thresholds,
    _verified_calibration_runs,
)
from .calibration_records import _calibration_bundle
from .contracts import sha256

def verify_calibration_bundle(
    path: Path,
    measurement_contract: dict,
    *,
    compare_frozen_constant_set: bool = True,
    frozen_source: dict | None = None,
    repository_root: Path | None = None,
    enforce_source_lineage: bool = False,
    expected_producer_run_id: str | None = None,
    expected_producer_artifact: str | None = None,
) -> dict:
    bundle = _calibration_bundle(
        path,
        measurement_contract,
        expected_producer_run_id=expected_producer_run_id,
        expected_producer_artifact=expected_producer_artifact,
    )
    accumulator = _verified_calibration_runs(bundle)
    selected_constants = _selected_calibration_constants(
        accumulator.duration_values_ms
    )
    thresholds = _selected_calibration_thresholds(
        accumulator.metric_values,
        bundle.protocol["metric_contracts"],
    )
    freeze_digest, source_lineage = _calibration_freeze(
        bundle,
        accumulator,
        selected_constants,
        thresholds,
        compare_frozen_constant_set=compare_frozen_constant_set,
        frozen_source=frozen_source,
        repository_root=repository_root,
        enforce_source_lineage=enforce_source_lineage,
    )
    return {
        "artifact": {"name": path.name, "sha256": sha256(path)},
        "source": bundle.source,
        "producer": bundle.producer,
        "contracts": bundle.contracts,
        "matrix_cell_count": len(bundle.matrix),
        "run_count": len(bundle.runs),
        "freeze_digest": freeze_digest,
        "calibration_required_values": selected_constants,
        "qualification_thresholds": thresholds,
        "run_artifact_sha256s": sorted(accumulator.artifact_digests),
        "source_lineage": source_lineage,
    }
