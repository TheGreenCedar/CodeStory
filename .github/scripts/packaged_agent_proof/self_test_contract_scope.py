"""Claim-scope and publication-identity contract self-tests."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from .contract_primitives import require_sha256, validate_runtime_claim_scope
from .foundation import ProofFailure, require
from .process_memory_sampling import parse_byte_quantity
from .publication_protocol import publication_identity_from_status
from .qualification_recording import record_qualification_contract


def _claim_scope_tests() -> None:
    require(
        parse_byte_quantity("24.1M") == 25_270_682,
        "memory quantity parser failed",
    )
    valid_ground_scope = argparse.Namespace(
        ground_only=True,
        server_behavior_only=False,
        version_only=False,
        plugin_handoff=True,
        project=Path("."),
        produce_qualification_evidence=False,
        qualification_evidence=None,
        retrieval_quality_evidence=None,
        publication_fault_evidence=None,
        proof_tier="installed_runtime",
    )
    validate_runtime_claim_scope(valid_ground_scope)
    for field, value in (
        ("plugin_handoff", False),
        ("project", None),
        ("server_behavior_only", True),
        ("produce_qualification_evidence", True),
        ("qualification_evidence", Path("qualification.json")),
        ("retrieval_quality_evidence", Path("quality.json")),
        ("publication_fault_evidence", Path("fault.json")),
    ):
        hostile_scope = argparse.Namespace(**vars(valid_ground_scope))
        setattr(hostile_scope, field, value)
        try:
            validate_runtime_claim_scope(hostile_scope)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"ground-only scope accepted incompatible {field}={value!r}"
            )
    summary = {
        "package_contract": {
            "release_readiness_claim": False,
            "highest_proof_tier": "package",
        }
    }
    record_qualification_contract(
        argparse.Namespace(
            ground_only=False,
            server_behavior_only=True,
            proof_tier="protected_hardware",
        ),
        summary,
        {},
        {},
        {},
    )
    require(
        summary["server_behavior"]["release_readiness_claim"] is True
        and summary["package_contract"]["release_readiness_claim"] is True,
        "bounded server proof recorded contradictory release-readiness claims",
    )


def _publication_identity_tests() -> None:
    publication_status = {
        "retrieval_mode": "full",
        "manifest_contract": {
            "project_id": "repo-v2-self-test",
            "input_hash": "1" * 64,
            "generation": "repo-v2-self-test-generation",
            "schema_version": 6,
            "graph_hash": "2" * 64,
        },
        "manifest": {
            "project_id": "repo-v2-self-test",
            "sidecar_input_hash": "1" * 64,
            "sidecar_generation": "repo-v2-self-test-generation",
            "sidecar_schema_version": 6,
            "graph_artifact_hash": "2" * 64,
            "lexical_version": "sqlite-fts5-v1",
            "semantic_generation": "semantic-self-test",
            "scip_revision": "graph-self-test",
        },
    }
    publication_identity = publication_identity_from_status(publication_status)
    require_sha256(publication_identity, "publication identity self-test")
    hostile_publication_status = json.loads(json.dumps(publication_status))
    hostile_publication_status["manifest"]["sidecar_generation"] = "stale-generation"
    try:
        publication_identity_from_status(hostile_publication_status)
    except ProofFailure:
        pass
    else:
        raise ProofFailure("manifest report/contract drift was accepted")


def run_contract_scope_self_tests() -> None:
    _claim_scope_tests()
    _publication_identity_tests()
