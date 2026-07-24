"""Typed retained qualification contracts and verified bindings."""

from __future__ import annotations

from dataclasses import dataclass

from .foundation import QUALIFICATION_SCHEMA_VERSION, require
from .measurement_samples import selected_qualification_matrix_cell


@dataclass(frozen=True)
class RetainedQualificationEvidence:
    raw: dict
    tier: str
    installed_plugin: dict | None
    managed_runtime: dict | None
    source: dict
    package: dict
    host: dict
    same_account: dict
    shared_identity: dict
    timing: dict
    scenarios: dict
    lower_tier_nonclaims: dict
    metrics: dict

    @classmethod
    def from_dict(cls, evidence: dict) -> RetainedQualificationEvidence:
        require(
            evidence.get("schema_version") == QUALIFICATION_SCHEMA_VERSION,
            "retained qualification schema is unsupported",
        )
        require(
            evidence.get("status") == "pass",
            "retained qualification is not a passing result",
        )
        tier = evidence.get("tier")
        require(
            tier in {"hosted_package", "protected_hardware", "installed_runtime"},
            "retained qualification tier is invalid",
        )
        installed_plugin = evidence.get("installed_plugin")
        managed_runtime = evidence.get("managed_runtime")
        if tier == "installed_runtime":
            require(
                isinstance(installed_plugin, dict),
                "installed evidence omitted plugin provenance",
            )
            require(
                isinstance(managed_runtime, dict),
                "installed evidence omitted managed runtime provenance",
            )
        else:
            require(
                installed_plugin is None and managed_runtime is None,
                "lower-tier evidence must not claim installed plugin provenance",
            )
        fields = {
            "source": "retained qualification omitted source identity",
            "package": "retained qualification omitted package identity",
            "host": "retained qualification omitted host identity",
            "same_account": "retained qualification omitted same-account evidence",
            "shared_identity": "retained qualification omitted shared server identity",
            "timing": "retained qualification omitted timing identity",
            "scenarios": "retained qualification omitted scenario evidence",
            "lower_tier_nonclaims": "retained qualification omitted lower-tier nonclaims",
            "metrics": "retained qualification omitted metric results",
        }
        normalized = {}
        for field, message in fields.items():
            value = evidence.get(field)
            require(isinstance(value, dict), message)
            normalized[field] = value
        return cls(
            evidence,
            tier,
            installed_plugin,
            managed_runtime,
            **normalized,
        )


@dataclass(frozen=True)
class RetainedQualificationContract:
    evidence: RetainedQualificationEvidence
    manifest: dict
    archive_sha256: str
    live_shared_identity: dict
    measurement_contract: dict
    required_tier: str
    matrix_cell_id: str
    expected_policy: str
    expected_backend: str
    accelerator_claim: str
    matrix_cell: dict
    live_installed_plugin: dict | None
    live_managed_runtime: dict | None

    @classmethod
    def create(
        cls,
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
        installed_plugin: dict | None,
        managed_runtime: dict | None,
    ) -> RetainedQualificationContract:
        retained = RetainedQualificationEvidence.from_dict(evidence)
        require(
            retained.tier == required_tier,
            f"retained {retained.tier} evidence cannot support exact requested tier {required_tier}",
        )
        matrix_cell = selected_qualification_matrix_cell(
            measurement_contract["measurement_protocol"],
            cell_id=required_matrix_cell_id,
            target=manifest["asset_target"],
            proof_tier=required_tier,
            expected_policy=expected_policy,
            expected_backend=expected_backend,
        )
        require(
            matrix_cell["accelerator_claim"] == expected_accelerator_claim,
            "requested accelerator claim does not match the selected qualification matrix cell",
        )
        return cls(
            retained,
            manifest,
            archive_sha256,
            shared_identity,
            measurement_contract,
            required_tier,
            required_matrix_cell_id,
            expected_policy,
            expected_backend,
            expected_accelerator_claim,
            matrix_cell,
            installed_plugin,
            managed_runtime,
        )


@dataclass(frozen=True)
class RetainedPackageBinding:
    source: dict
    package: dict
    matrix_cell: dict


@dataclass(frozen=True)
class RetainedRuntimeBinding:
    installed_plugin: dict | None
    managed_runtime: dict | None
    host: dict
    same_account: dict
    shared_identity: dict
    timing: dict


@dataclass(frozen=True)
class RetainedMetric:
    name: str
    value: int | float
    threshold: int | float
    comparison: str
    raw_evidence: dict


@dataclass(frozen=True)
class RetainedMeasurementBinding:
    scenarios: dict
    lower_tier_nonclaims: dict
    metrics: tuple[RetainedMetric, ...]


@dataclass(frozen=True)
class VerifiedRetainedQualification:
    parsed: RetainedQualificationEvidence
    package: RetainedPackageBinding
    runtime: RetainedRuntimeBinding
    measurements: RetainedMeasurementBinding
