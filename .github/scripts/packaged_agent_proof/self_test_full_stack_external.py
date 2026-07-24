"""External publication, recovery, and retrieval evidence self-tests."""

from __future__ import annotations

import hashlib
import json

from .contracts import (
    load_holdout_task_contracts,
    verify_package_server_contracts,
    write_json,
)
from .foundation import (
    FAULT_RECOVERY_CONSISTENCY_CASES,
    FAULT_RECOVERY_CONSISTENCY_CONTRACT,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    PUBLICATION_FAULT_EVIDENCE_CONTRACT,
    RELEASE_QUALITY_CORPUS_ID,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    ProofFailure,
    require,
)
from .qualification_scenarios import derive_scenario_assertions
from .runtime import (
    verify_fault_recovery_consistency_raw_evidence,
    verify_publication_fault_raw_evidence,
    verify_retrieval_quality_raw_evidence,
)
from .self_test_full_stack_types import (
    ExternalEvidenceFixture,
    FullStackFixture,
    PublicationFixture,
    QualityFixture,
)


def _external_contracts(fixture: FullStackFixture) -> tuple[dict, dict]:
    return (
        {
            "archive_sha256": "b" * 64,
            "executable_sha256": fixture.manifest["binary"]["sha256"],
            "asset_target": fixture.manifest["asset_target"],
            "release_version": fixture.manifest["release_version"],
        },
        {
            "protocol_sha256": fixture.protocol_sha256,
            "constant_set_sha256": fixture.constant_set_sha256,
            "measurement_protocol_sha256": fixture.measurement_protocol_sha256,
        },
    )


def _publication_fault_test(
    fixture: FullStackFixture,
) -> PublicationFixture:
    root = fixture.root
    manifest = fixture.manifest
    external_package, external_contracts = _external_contracts(fixture)
    self_digest = lambda label: hashlib.sha256(label.encode("utf-8")).hexdigest()
    correlation_id = "0123456789abcdef0123456789abcdef"
    previous_publication = self_digest("previous-publication")
    previous_publication = self_digest("previous-publication")
    publication_payload = {
        "schema_version": 1,
        "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "source": manifest["source"],
        "package": external_package,
        "contracts": external_contracts,
        "correlation_id": correlation_id,
        "previous_publication_identity_sha256": previous_publication,
        "server_observations": [
            {
                "phase": "before_crash",
                "server_instance_id": "server-before",
                "process_start_id": "boot-1:101",
                "load_generation": 1,
            },
            {
                "phase": "after_replacement",
                "server_instance_id": "server-after",
                "process_start_id": "boot-1:102",
                "load_generation": 1,
            },
        ],
        "candidate_observation": {
            "command": "retrieval_index",
            "exit_code": 1,
            "stdout_sha256": self_digest("candidate-stdout"),
            "stderr_sha256": self_digest("candidate-stderr"),
        },
        "publication_hook_events": [
            {
                "schema_version": 1,
                "sequence": index,
                "correlation_id": correlation_id,
                "action": action,
                "status": status,
                "clock": {
                    "domain": "process_monotonic",
                    "api": "std::time::Instant",
                    "elapsed_ns": index,
                },
            }
            for index, (action, status) in enumerate(
                (
                    ("pause_before_manifest_commit", "waiting_for_resume"),
                    ("resume_manifest_commit", "observed"),
                    ("lease_revalidation", "failed"),
                    ("manifest_commit", "returned_error"),
                )
            )
        ],
        "ordinary_product_observations": [
            {
                "sequence": index,
                "command": command,
                "exit_code": 0,
                "retrieval_mode": "full",
                "publication_identity_sha256": previous_publication,
                "output_sha256": self_digest(f"{command}-output"),
            }
            for index, command in enumerate(("retrieval_status", "search"))
        ],
    }
    publication_path = root / "publication-fault.raw.json"
    write_json(publication_path, publication_payload)
    publication_external = verify_publication_fault_raw_evidence(
        publication_path,
        source=manifest["source"],
        package=external_package,
        contracts=external_contracts,
    )
    require(
        all(publication_external["assertions"].values()),
        "publication fault self-test did not derive its assertions",
    )
    return PublicationFixture(
        path=publication_path,
        payload=publication_payload,
        external=publication_external,
    )


def _publication_fault_hostile(
    fixture: FullStackFixture,
    publication: PublicationFixture,
) -> None:
    manifest = fixture.manifest
    external_package, external_contracts = _external_contracts(fixture)
    publication_path = publication.path
    publication_payload = publication.payload
    hostile_publication = json.loads(json.dumps(publication_payload))
    hostile_publication["assertions"] = {"lost_publication_lease_blocks_commit": True}
    write_json(publication_path, hostile_publication)
    try:
        verify_publication_fault_raw_evidence(
            publication_path,
            source=manifest["source"],
            package=external_package,
            contracts=external_contracts,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("self-declared publication assertions were accepted")
    write_json(publication_path, publication_payload)


def _server_crash_scenario_tests() -> None:
    scenario_observations = {
        "inflight_request_observed": [
            {
                "values": {
                    "query_capacity": 64,
                    "query_depth": 0,
                    "bulk_capacity": 64,
                    "bulk_depth": 0,
                    "active_request_count": 1,
                    "lease_count": 0,
                    "active_request_class": "query",
                }
            }
        ],
        "server_replaced": [
            {
                "values": {
                    "old_server_instance_id": "server-before",
                    "new_server_instance_id": "server-after",
                }
            }
        ],
        "query_replayed": [
            {
                "values": {
                    "logical_operation_count": 1,
                    "wire_attempt_count": 2,
                    "wire_attempts": [
                        {
                            "ordinal": 1,
                            "request_id": "request-before",
                            "server_instance_id": "server-before",
                            "submitted_ns": 1,
                            "completed_ns": 2,
                            "outcome": "server_loss",
                        },
                        {
                            "ordinal": 2,
                            "request_id": "request-after",
                            "server_instance_id": "server-after",
                            "submitted_ns": 3,
                            "completed_ns": 4,
                            "outcome": "completed",
                        },
                    ],
                }
            }
        ],
    }
    derived_server_crash = derive_scenario_assertions(
        "server_crash",
        observations_by_kind=scenario_observations,
        process_observations=[],
        invocations=[],
        same_account={},
        materialization={},
    )
    require(
        derived_server_crash
        == {
            "one_replacement_server": True,
            "pure_embedding_rpc_replayed_at_most_once": True,
        },
        "scenario assertion self-test did not derive exact raw claims",
    )
    hostile_scenario = json.loads(json.dumps(scenario_observations))
    hostile_scenario["query_replayed"][0]["values"]["wire_attempts"][1]["outcome"] = (
        "server_loss"
    )
    try:
        derive_scenario_assertions(
            "server_crash",
            observations_by_kind=hostile_scenario,
            process_observations=[],
            invocations=[],
            same_account={},
            materialization={},
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("named scenario transitions with false values were accepted")


def _fault_consistency_tests(fixture: FullStackFixture) -> dict:
    root = fixture.root
    manifest = fixture.manifest
    external_package, external_contracts = _external_contracts(fixture)
    self_digest = lambda label: hashlib.sha256(label.encode("utf-8")).hexdigest()
    consistency_payload = {
        "schema_version": 1,
        "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "source": manifest["source"],
        "package": external_package,
        "contracts": external_contracts,
        "run_id_sha256": self_digest("consistency-run"),
        "observations": [
            {
                "case_id_sha256": self_digest(f"consistency-case-{index}"),
                "before_server_fault_rank": 1,
                "after_server_replacement_rank": 1,
            }
            for index in range(FAULT_RECOVERY_CONSISTENCY_CASES)
        ],
    }
    consistency_path = root / "fault-recovery-consistency.raw.json"
    write_json(consistency_path, consistency_payload)
    consistency_external = verify_fault_recovery_consistency_raw_evidence(
        consistency_path,
        source=manifest["source"],
        package=external_package,
        contracts=external_contracts,
    )
    require(
        consistency_external["ranks_stable"] is True,
        "fault recovery consistency self-test did not derive stable ranks",
    )
    hostile_consistency = json.loads(json.dumps(consistency_payload))
    hostile_consistency["observations"][0]["after_server_replacement_rank"] = 2
    write_json(consistency_path, hostile_consistency)
    try:
        verify_fault_recovery_consistency_raw_evidence(
            consistency_path,
            source=manifest["source"],
            package=external_package,
            contracts=external_contracts,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("changed fault-recovery search ranks were accepted")
    return consistency_external


def _quality_payload(fixture: FullStackFixture) -> dict:
    manifest = fixture.manifest
    packet_row = {
        "repo": "fixture",
        "task_id": "quality-contract",
        "mode": "cold_cli_packet",
        "status": "pass",
        "quality": {"pass": True},
        "sufficiency": {
            "status": "sufficient",
            "sufficient_quality_mismatch": False,
            "follow_up_commands_count": 0,
            "open_next_count": 0,
            "gaps_count": 0,
            "coverage_unresolved_blocking_count": 0,
        },
        "packet_latency": {
            "sla_missed": False,
            "retrieval_shadow": {"retrieval_mode": "full"},
        },
        "repo_provenance": {
            "manifest_overridden_by_builtin": False,
            "configured": {
                "url": "https://github.com/example/fixture.git",
                "ref": "9" * 40,
            },
            "manifest": {
                "url": "https://github.com/example/fixture.git",
                "ref": "9" * 40,
            },
            "git_head": "9" * 40,
            "git_origin": "https://github.com/example/fixture.git",
            "git_dirty": False,
        },
        "codestory_cache_provenance": {
            "doctor_status": "pass",
            "storage_path": "fixture/codestory.db",
            "cache_policy": "prepared-retrieval-cache-read-only",
            "retrieval_mode": "full",
            "semantic_generation": "fixture-generation",
            "manifest_embedding_backend": "per-user-server:coderank-embed:q8_0",
            "semantic_backend": "coderank-embed",
            "embedding_engine_instance_id": "engine-fixture",
            "embedding_policy": "accelerated",
            "local_only": True,
            "indexed": True,
            "freshness_status": "fresh",
            "semantic_ready": True,
            "indexing_in_timed_run": False,
        },
    }
    holdout_tasks, _holdout_manifest_set_sha256 = load_holdout_task_contracts()
    quality_rows = []
    for (repo_name, task_id), task_contract in sorted(holdout_tasks.items()):
        for repeat in range(1, MIN_RETRIEVAL_QUALITY_REPEATS + 1):
            row = json.loads(json.dumps(packet_row))
            row["repo"] = repo_name
            row["task_id"] = task_id
            row["repeat"] = repeat
            row["task_manifest_snapshot"] = {
                **task_contract["snapshot"],
                "manifest_path": str(task_contract["path"]),
            }
            row["repo_provenance"]["configured"] = {
                "url": task_contract["repo"]["url"],
                "ref": task_contract["repo"]["ref"],
                "languages": task_contract["repo"].get("languages", []),
            }
            row["repo_provenance"]["manifest"] = {
                "url": task_contract["repo"]["url"],
                "ref": task_contract["repo"]["ref"],
                "workspace_root": task_contract["repo"].get("workspace_root"),
            }
            row["repo_provenance"]["git_head"] = task_contract["repo"]["ref"]
            row["repo_provenance"]["git_origin"] = task_contract["repo"]["url"]
            quality_rows.append(row)
    quality_payload = {
        "modes": ["cold-cli"],
        "repeats": MIN_RETRIEVAL_QUALITY_REPEATS,
        "release_evidence": {
            "commit": manifest["source"]["commit"],
            "source_tree": manifest["source"]["tree"],
            "evaluation_contract": RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
            "profile": "self-test",
            "evidence_identity": {
                "corpus_id": RELEASE_QUALITY_CORPUS_ID,
                "cache_id": "self-test-cache",
                "machine_fingerprint": "self-test-host",
            },
            "publishable": True,
            "repeats": MIN_RETRIEVAL_QUALITY_REPEATS,
            "quality_gate_status": "pass",
            "publishable_blockers": [],
            "rows": quality_rows,
        },
    }
    return quality_payload


def _verified_quality(
    fixture: FullStackFixture,
    quality_payload: dict,
) -> QualityFixture:
    root = fixture.root
    manifest = fixture.manifest
    quality_path = root / "packet-runtime-summary.json"
    write_json(quality_path, quality_payload)
    quality_external = verify_retrieval_quality_raw_evidence(
        quality_path,
        source=manifest["source"],
    )
    require(
        quality_external["publishable_packet_pass_rate"] == 1,
        "retrieval quality self-test did not derive the packet pass rate",
    )
    return QualityFixture(
        path=quality_path,
        payload=quality_payload,
        external=quality_external,
    )


def _quality_hostiles(
    fixture: FullStackFixture,
    quality: QualityFixture,
) -> None:
    manifest = fixture.manifest
    self_measurement_protocol = fixture.measurement_protocol
    quality_path = quality.path
    quality_payload = quality.payload
    hostile_quality = json.loads(json.dumps(quality_payload))
    hostile_quality["assertions"] = {"retrieval_quality": True}
    write_json(quality_path, hostile_quality)
    try:
        verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("self-declared retrieval quality pass was accepted")
    hostile_quality = json.loads(json.dumps(quality_payload))
    hostile_quality["release_evidence"]["rows"].pop()
    write_json(quality_path, hostile_quality)
    try:
        verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("incomplete retrieval quality repeats were accepted")
    hostile_quality = json.loads(json.dumps(quality_payload))
    hostile_quality["release_evidence"]["rows"] = [
        row
        for row in hostile_quality["release_evidence"]["rows"]
        if row["task_id"] == "axios-request-dispatch"
    ]
    write_json(quality_path, hostile_quality)
    try:
        verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("one-task retrieval quality subset was accepted")
    hostile_quality = json.loads(json.dumps(quality_payload))
    hostile_quality["release_evidence"]["rows"][0]["task_manifest_snapshot"][
        "prompt"
    ] = "hostile substituted task"
    write_json(quality_path, hostile_quality)
    try:
        verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("substituted retrieval quality task manifest was accepted")
    hostile_quality = json.loads(json.dumps(quality_payload))
    hostile_quality["release_evidence"]["source_tree"] = "f" * 40
    write_json(quality_path, hostile_quality)
    try:
        verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("stale retrieval quality source tree was accepted")
    write_json(quality_path, quality_payload)
    try:
        verify_package_server_contracts(
            manifest,
            self_measurement_protocol,
            require_frozen=True,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("unfrozen server constants were accepted for qualification")


def run_external_evidence_self_tests(
    fixture: FullStackFixture,
) -> ExternalEvidenceFixture:
    publication = _publication_fault_test(fixture)
    _publication_fault_hostile(fixture, publication)
    _server_crash_scenario_tests()
    consistency = _fault_consistency_tests(fixture)
    quality = _verified_quality(fixture, _quality_payload(fixture))
    _quality_hostiles(fixture, quality)
    return ExternalEvidenceFixture(
        publication=publication.external,
        consistency=consistency,
        quality=quality.external,
    )
