"""Claim-scope and publication-identity contract self-tests."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from unittest.mock import Mock, call, patch

from . import ground_proof
from .contract_primitives import require_sha256, validate_runtime_claim_scope
from .foundation import ProofFailure, require
from .process_memory_sampling import parse_byte_quantity
from .publication_protocol import publication_identity_from_status
from .qualification_recording import record_qualification_contract
from .runtime_contract import record_runtime_contract


def _single_project_host() -> Mock:
    host = Mock()
    host.tool_until_ready.return_value = (
        {"result": {"structuredContent": {"root": "self-test"}}},
        1,
    )
    host.search_until_ready.return_value = ({}, 2)
    host.engine_diagnostics.return_value = {"self_test": "engine-diagnostics"}
    return host


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
        additional_project=[],
        additional_query=[],
        produce_qualification_evidence=False,
        qualification_evidence=None,
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
        {
            "ground": {"project_bound": True},
            "search": {"project_bound": True, "retrieval_ready": True},
        },
        {},
    )
    require(
        summary["server_behavior"]["release_readiness_claim"] is True
        and summary["server_behavior"]["project_bound"] is True
        and summary["server_behavior"]["retrieval_ready"] is True
        and summary["server_behavior"]["shared_server_claim"] is False
        and summary["package_contract"]["release_readiness_claim"] is True
        and summary["package_contract"]["highest_proof_tier"]
        == "protected_hardware",
        "bounded server proof recorded contradictory release-readiness claims",
    )


def _single_project_mode_tests() -> None:
    project = Path.cwd().resolve()
    plugin_root = project / "plugins" / "codestory"
    cli = project / "target" / "release" / "codestory-cli"
    args = argparse.Namespace(
        proof_tier="protected_hardware",
        query="single_project_probe",
        engine_policy="accelerated",
        expected_backend="Vulkan",
    )
    host = _single_project_host()
    attempts, managed_runtime, managed_binary = ground_proof._run_ground(
        args,
        host,
        project,
        plugin_root,
        cli,
        {},
        None,
    )
    require(
        host.method_calls
        == [
            call.initialize(),
            call.tool_until_ready(
                "ground",
                {"project": str(project), "budget": "strict"},
                "installed-ground",
            ),
        ]
        and attempts == 1
        and managed_runtime is None
        and managed_binary is None,
        "ground-only execution grew beyond one project-bound ground request",
    )
    ground = ground_proof._ground_result(
        attempts=attempts,
        provenance=None,
        managed_runtime=None,
        managed_binary=None,
        cli=cli,
        project=project,
        plugin_root=plugin_root,
        root=project,
        qualified_env={
            "CODESTORY_EMBED_QUALIFICATION_DIR": str(project / "qualification"),
            "CODESTORY_EMBED_QUALIFICATION_NONCE": "self-test-nonce",
        },
    )
    require(
        not {"search", "identity", "snapshot", "runtime_evidence"}.intersection(ground)
        and "release_readiness" in ground["nonclaims"]
        and "linux_gpu_execution" in ground["nonclaims"],
        "ground-only evidence grew search, server, identity, or accelerator claims",
    )
    ground_summary = {"package_contract": {}}
    record_runtime_contract(
        argparse.Namespace(
            ground_only=True,
            server_behavior_only=False,
            proof_tier="protected_hardware",
            engine_policy="accelerated",
        ),
        ground_summary,
        {},
        ground,
    )
    require(
        ground_summary["ground_receipt"]["accelerator_claim"] is False
        and "runtime_evidence" not in ground_summary["package_contract"],
        "ground-only receipt acquired native runtime or accelerator evidence",
    )

    server_host = _single_project_host()
    ground_proof._run_ground(
        args,
        server_host,
        project,
        plugin_root,
        cli,
        {},
        None,
    )
    identity = {"embedding_policy": "accelerated"}
    snapshot = {"schema_version": 1}
    manifest = {"self_test": "manifest"}
    with (
        patch.object(ground_proof, "engine_identity", return_value=identity) as identity_check,
        patch.object(ground_proof, "server_snapshot", return_value=snapshot) as snapshot_check,
    ):
        server = ground_proof._prove_server_readiness(
            args,
            server_host,
            project,
            manifest,
        )
    require(
        server_host.method_calls
        == [
            call.initialize(),
            call.tool_until_ready(
                "ground",
                {"project": str(project), "budget": "strict"},
                "installed-ground",
            ),
            call.search_until_ready(
                {
                    "project": str(project),
                    "query": "single_project_probe",
                },
                "server-readiness-search",
            ),
            call.engine_diagnostics(project, "server-readiness-diagnostics"),
        ],
        "server-behavior proof did not ground, search, and inspect one project in order",
    )
    require(
        server
        == {
            "search": {
                "status": "pass",
                "attempts": 2,
                "project_bound": True,
                "retrieval_ready": True,
            },
            "identity": identity,
            "snapshot": snapshot,
        }
        and identity_check.call_args.args
        == (server_host.engine_diagnostics.return_value, "accelerated", "Vulkan")
        and snapshot_check.call_args.args
        == (server_host.engine_diagnostics.return_value, manifest)
        and snapshot_check.call_args.kwargs == {"require_resident": True},
        "server-behavior proof omitted same-project search or native identity evidence",
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

    # Every contract->manifest pairing is bound one field at a time: dropping
    # any one equality check must turn that field's drift invisible, so each
    # divergence is an independent negative.
    for contract_field, manifest_field in (
        ("project_id", "project_id"),
        ("generation", "sidecar_generation"),
        ("input_hash", "sidecar_input_hash"),
        ("schema_version", "sidecar_schema_version"),
        ("graph_hash", "graph_artifact_hash"),
    ):
        hostile = json.loads(json.dumps(publication_status))
        original = hostile["manifest"][manifest_field]
        hostile["manifest"][manifest_field] = (
            "stale-generation" if isinstance(original, str)
            else original + 1
        )
        try:
            publication_identity_from_status(hostile)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"manifest report/contract drift in {manifest_field} was accepted"
            )

    # Each identity-payload field must change the digest when it changes on
    # both sides; a field dropped from the canonical payload stops binding it.
    for contract_field, manifest_field, hostile_value in (
        ("project_id", "project_id", "repo-v2-other-project"),
        ("generation", "sidecar_generation", "repo-v2-other-generation"),
        ("input_hash", "sidecar_input_hash", "9" * 64),
        ("schema_version", "sidecar_schema_version", 7),
        ("graph_hash", "graph_artifact_hash", "8" * 64),
        (None, "lexical_version", "sqlite-fts5-v2"),
        (None, "semantic_generation", "semantic-other"),
        (None, "scip_revision", "graph-other"),
    ):
        consistent = json.loads(json.dumps(publication_status))
        consistent["manifest"][manifest_field] = hostile_value
        if contract_field is not None:
            consistent["manifest_contract"][contract_field] = hostile_value
        try:
            shifted = publication_identity_from_status(consistent)
        except ProofFailure as error:
            raise ProofFailure(
                f"a consistent {manifest_field} change was rejected: {error}"
            ) from error
        require(
            shifted != publication_identity,
            f"the publication identity does not bind {manifest_field}",
        )


def run_contract_scope_self_tests() -> None:
    _claim_scope_tests()
    _single_project_mode_tests()
    _publication_identity_tests()
