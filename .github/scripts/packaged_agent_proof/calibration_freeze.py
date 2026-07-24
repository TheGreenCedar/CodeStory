"""Calibration freeze digest and source-binding verification."""

from __future__ import annotations

from pathlib import Path

from .calibration_lineage import verify_calibration_source_lineage
from .calibration_records import CalibrationAccumulator, CalibrationBundle
from .contracts import canonical_sha256, sha256
from .foundation import require

def _calibration_freeze(
    bundle: CalibrationBundle,
    accumulator: CalibrationAccumulator,
    selected_constants: dict,
    thresholds: dict,
    *,
    compare_frozen_constant_set: bool,
    frozen_source: dict | None,
    repository_root: Path | None,
    enforce_source_lineage: bool,
) -> tuple[str, dict | None]:
    if compare_frozen_constant_set:
        require(
            bundle.constant_set["calibration_required_values"] == selected_constants,
            "frozen compiled constants do not match the preregistered calibration formulas",
        )
        require(
            bundle.constant_set["qualification_thresholds"] == thresholds,
            "frozen qualification thresholds do not match the preregistered calibration formulas",
        )
    digests = sorted(accumulator.artifact_digests)
    freeze_digest = canonical_sha256(
        {
            "selection_protocol": bundle.raw["selection_protocol"],
            "source": bundle.source,
            "producer": bundle.producer,
            "contracts": bundle.contracts,
            "run_artifact_sha256s": digests,
            "calibration_required_values": selected_constants,
            "qualification_thresholds": thresholds,
        }
    )
    lineage = None
    if compare_frozen_constant_set and enforce_source_lineage:
        require(
            frozen_source is not None and repository_root is not None,
            "frozen qualification requires exact calibration-to-package source lineage",
        )
        lineage = verify_calibration_source_lineage(
            bundle.source,
            frozen_source,
            repository_root,
        )
    if compare_frozen_constant_set:
        _verify_calibration_freeze_record(
            bundle,
            digests,
            freeze_digest,
        )
    return freeze_digest, lineage


def _verify_calibration_freeze_record(
    bundle: CalibrationBundle,
    digests: list[str],
    freeze_digest: str,
) -> None:
    require(
        bundle.raw["freeze_digest"] == freeze_digest,
        "calibration bundle freeze digest does not match recomputed raw evidence",
    )
    record = bundle.constant_set["freeze_record"]
    require(
        record["selection_source_commit"] == bundle.source["commit"]
        and record["selection_source_tree"] == bundle.source["tree"]
        and record["measurement_protocol_sha256"]
        == bundle.contracts["measurement_protocol_sha256"]
        and record["protocol_sha256"] == bundle.contracts["protocol_sha256"]
        and record["input_constant_set_sha256"]
        == bundle.contracts["input_constant_set_sha256"]
        and record["calibration_bundle_sha256"] == sha256(bundle.path)
        and record["calibration_freeze_digest"] == freeze_digest
        and sorted(record["run_artifact_sha256s"]) == digests
        and record["selection_rule"]
        == "all_preregistered_clean_runs_no_outlier_removal",
        "constant-set freeze record does not bind the exact recomputed calibration bundle",
    )
