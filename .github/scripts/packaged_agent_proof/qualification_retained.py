"""Verification entry point for retained qualification evidence."""

from __future__ import annotations

from .qualification_retained_metrics import _verify_measurement_binding
from .qualification_retained_provenance import _verify_runtime_binding, _verify_source_and_package
from .qualification_retained_types import RetainedQualificationContract, VerifiedRetainedQualification

def verify_retained_qualification(
    evidence: dict,
    *,
    manifest: dict,
    archive_sha256: str,
    shared_identity: dict,
    measurement_contract: dict,
    required_tier: str,
    required_matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    expected_accelerator_claim: str,
    installed_plugin: dict | None = None,
    managed_runtime: dict | None = None,
) -> dict:
    contract = RetainedQualificationContract.create(
        evidence,
        manifest=manifest,
        archive_sha256=archive_sha256,
        shared_identity=shared_identity,
        measurement_contract=measurement_contract,
        required_tier=required_tier,
        required_matrix_cell_id=required_matrix_cell_id,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
        expected_accelerator_claim=expected_accelerator_claim,
        installed_plugin=installed_plugin,
        managed_runtime=managed_runtime,
    )
    verified = VerifiedRetainedQualification(
        parsed=contract.evidence,
        package=_verify_source_and_package(contract),
        runtime=_verify_runtime_binding(contract),
        measurements=_verify_measurement_binding(contract),
    )
    return verified.parsed.raw
