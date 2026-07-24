"""Assembly and freezing for calibration bundles."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from .foundation import (
    ProofFailure,
    require,
)
from .contracts import (
    load_server_measurement_contract,
    require_nonempty_string,
    sha256,
    write_json,
)
from .calibration_verification import verify_calibration_bundle

def _calibration_run_documents(
    paths: list[Path],
    *,
    expected_count: int,
) -> list[dict]:
    resolved = [path.resolve() for path in paths]
    require(
        len(resolved) == expected_count and len(set(resolved)) == expected_count,
        f"calibration assembly requires exactly {expected_count} distinct run artifacts",
    )
    runs = []
    for position, path in enumerate(resolved):
        require(
            path.is_file() and not path.is_symlink(),
            f"calibration run artifact {position} is missing or unsafe: {path}",
        )
        try:
            run = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise ProofFailure(
                f"calibration run artifact {position} is invalid JSON: {exc}"
            ) from exc
        require(
            isinstance(run, dict),
            f"calibration run artifact {position} must be an object",
        )
        runs.append(run)
    return runs


def _unfrozen_calibration_bundle(
    args: argparse.Namespace,
    measurement_contract: dict,
    runs: list[dict],
) -> tuple[dict, dict, dict]:
    constant_set = measurement_contract["constant_set"]
    require(
        constant_set.get("status") == "unfrozen"
        and constant_set.get("freeze_record") is None,
        "calibration assembly requires the exact unfrozen input constant set",
    )
    source = runs[0].get("source")
    contracts = runs[0].get("contracts")
    require(
        isinstance(source, dict) and isinstance(contracts, dict),
        "first calibration run omitted source or contract identity",
    )
    return (
        {
            "schema_version": 1,
            "selection_protocol": constant_set["selection_protocol"],
            "source": source,
            "producer": {
                "repository": args.calibration_producer_repository,
                "workflow_path": args.calibration_producer_workflow_path,
                "run_id": args.calibration_producer_run_id,
                "run_attempt": args.calibration_producer_run_attempt,
                "artifact_name": args.calibration_producer_artifact,
                "source_head_sha": source.get("commit"),
            },
            "contracts": contracts,
            "runs": runs,
            "freeze_digest": "",
        },
        source,
        contracts,
    )


def _frozen_calibration_constant_set(
    constant_set: dict,
    *,
    source: dict,
    contracts: dict,
    selection: dict,
    bundle_output: Path,
    selected_at: object,
) -> dict:
    frozen = json.loads(json.dumps(constant_set))
    frozen["status"] = "frozen"
    frozen["calibration_required_values"] = selection[
        "calibration_required_values"
    ]
    frozen["qualification_thresholds"] = selection["qualification_thresholds"]
    frozen["freeze_record"] = {
        "selection_source_commit": source["commit"],
        "selection_source_tree": source["tree"],
        "measurement_protocol_sha256": contracts["measurement_protocol_sha256"],
        "protocol_sha256": contracts["protocol_sha256"],
        "input_constant_set_sha256": contracts["input_constant_set_sha256"],
        "calibration_bundle_sha256": sha256(bundle_output),
        "calibration_freeze_digest": selection["freeze_digest"],
        "run_artifact_sha256s": selection["run_artifact_sha256s"],
        "selection_rule": "all_preregistered_clean_runs_no_outlier_removal",
        "selected_at": require_nonempty_string(
            selected_at,
            "--freeze-selected-at",
        ),
    }
    return frozen


def assemble_calibration_bundle(args: argparse.Namespace) -> dict:
    require(
        args.calibration_bundle_output is not None
        and args.frozen_constant_set_output is not None
        and args.freeze_selected_at is not None,
        "calibration assembly requires bundle output, frozen constant-set output, and selected-at",
    )
    measurement_contract = load_server_measurement_contract(
        args.measurement_protocol
    )
    expected_run_count = (
        len(measurement_contract["measurement_protocol"]["calibration_matrix"]) * 3
    )
    runs = _calibration_run_documents(
        args.calibration_run,
        expected_count=expected_run_count,
    )
    bundle, source, contracts = _unfrozen_calibration_bundle(
        args,
        measurement_contract,
        runs,
    )
    bundle_output = args.calibration_bundle_output.resolve()
    constant_output = args.frozen_constant_set_output.resolve()
    require(
        bundle_output != constant_output,
        "calibration bundle and frozen constant-set outputs must be distinct",
    )
    write_json(bundle_output, bundle)
    selection = verify_calibration_bundle(
        bundle_output,
        measurement_contract,
        compare_frozen_constant_set=False,
    )
    bundle["freeze_digest"] = selection["freeze_digest"]
    write_json(bundle_output, bundle)

    frozen_constant_set = _frozen_calibration_constant_set(
        measurement_contract["constant_set"],
        source=source,
        contracts=contracts,
        selection=selection,
        bundle_output=bundle_output,
        selected_at=args.freeze_selected_at,
    )
    write_json(constant_output, frozen_constant_set)
    frozen_contract = json.loads(json.dumps(measurement_contract))
    frozen_contract["constant_set"] = frozen_constant_set
    frozen_contract["constant_set_sha256"] = sha256(constant_output)
    verified = verify_calibration_bundle(
        bundle_output,
        frozen_contract,
        enforce_source_lineage=False,
    )
    return {
        "bundle": verified["artifact"],
        "frozen_constant_set": {
            "name": constant_output.name,
            "sha256": sha256(constant_output),
        },
        "selection_source": source,
        "run_count": verified["run_count"],
        "matrix_cell_count": verified["matrix_cell_count"],
        "freeze_digest": verified["freeze_digest"],
    }

