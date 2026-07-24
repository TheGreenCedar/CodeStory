"""Self Test for packaged CodeStory proof."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from .foundation import (
    ProofFailure,
    project_node_resource_uri,
    require,
    resource_uri_matches,
)
from .contracts import (
    assert_retained_json_privacy,
    canonical_sha256,
    load_holdout_task_contracts,
    require_sha256,
    selected_qualification_matrix_cell,
    sha256,
    validate_runtime_claim_scope,
    verify_package_server_contracts,
    write_json,
)
from .archive import (
    embedding_contract_digest,
    expected_archive_digest,
    find_cli,
    load_native_manifest,
    parse_server_proof_identity,
    unpack_archive,
    verify_runtime_against_manifest,
)
from .process import (
    ExactProcessExitWaiter,
    FailurePreservingTemporaryDirectory,
    McpProcess,
    engine_identity,
    extract_resource,
    live_process_executable_sha256,
    native_server_exit_wait_budget,
    native_server_exit_wait_required,
    parse_byte_quantity,
    process_start_identity,
    remaining_native_server_exit_wait_ms,
    require_native_process_start_identity,
    retained_final_native_server_exit_evidence,
    run,
    server_snapshot,
    shared_server_identity,
    verified_live_executable,
)
from .installation import (
    run_parallel,
)
from .runtime import (
    publication_identity_from_status,
    verify_fault_recovery_consistency_raw_evidence,
    verify_publication_fault_raw_evidence,
    verify_retrieval_quality_raw_evidence,
)
from .qualification import (
    derive_scenario_assertions,
    require_candidate_matrix_installation_source,
    verify_retained_qualification,
)
from .calibration import (
    assemble_calibration_bundle,
    build_calibration_self_test_bundle,
    verify_calibration_bundle,
)

def run_contract_self_tests() -> None:
    require(parse_byte_quantity("24.1M") == 25_270_682, "memory quantity parser failed")
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
    uri_project = Path("proof root")
    node_uri = project_node_resource_uri(
        "codestory://snippet",
        "node/id %1",
        uri_project,
    )
    require(
        node_uri
        == "codestory://snippet/node%2Fid%20%251?project=proof%20root",
        f"project-bound node resource URI encoding drifted: {node_uri}",
    )
    require(
        extract_resource(
            {
                "result": {
                    "contents": [
                        {
                            "uri": node_uri,
                            "text": json.dumps({"node": {"id": "node/id %1"}}),
                        }
                    ]
                }
            },
            node_uri,
        )
        == {"node": {"id": "node/id %1"}},
        "project-bound named resource extraction failed",
    )
    short_windows_resource = (
        "codestory://diagnostics/retrieval-engine"
        "?project=C%3A%2FUsers%2FRUNNER~1%2FAppData%2FLocal%2FTemp%2Fproof"
    )
    long_windows_resource = (
        "codestory://diagnostics/retrieval-engine"
        "?project=C%3A%2FUsers%2Frunneradmin%2FAppData%2FLocal%2FTemp%2Fproof"
    )
    identity_probes: list[tuple[Path, Path]] = []

    def same_windows_resource(left: Path, right: Path) -> bool:
        identity_probes.append((left, right))
        return True

    expected_identity_probe = (
        Path("C:/Users/RUNNER~1/AppData/Local/Temp/proof"),
        Path("C:/Users/runneradmin/AppData/Local/Temp/proof"),
    )
    require(
        extract_resource(
            {
                "result": {
                    "contents": [
                        {
                            "uri": long_windows_resource,
                            "text": json.dumps({"native_alias": True}),
                        }
                    ]
                }
            },
            short_windows_resource,
            platform_name="nt",
            samefile=same_windows_resource,
        )
        == {"native_alias": True}
        and identity_probes == [expected_identity_probe]
        and expected_identity_probe[0] != expected_identity_probe[1],
        "native-identical Windows project resource URI was rejected",
    )
    short_windows_snippet = short_windows_resource.replace(
        "codestory://diagnostics/retrieval-engine",
        "codestory://snippet/node%2Fid",
    )
    long_windows_snippet = long_windows_resource.replace(
        "codestory://diagnostics/retrieval-engine",
        "codestory://snippet/node%2Fid",
    )
    snippet_identity_probes: list[tuple[Path, Path]] = []

    def same_windows_snippet(left: Path, right: Path) -> bool:
        snippet_identity_probes.append((left, right))
        return True

    require(
        resource_uri_matches(
            short_windows_snippet,
            long_windows_snippet,
            platform_name="nt",
            samefile=same_windows_snippet,
        )
        and snippet_identity_probes == [expected_identity_probe],
        "native-identical Windows snippet link URI was rejected",
    )
    require(
        not resource_uri_matches(
            short_windows_resource,
            long_windows_resource,
            platform_name="posix",
            samefile=same_windows_resource,
        )
        and len(identity_probes) == 1,
        "Unix project resource matching accepted a different path spelling",
    )
    require(
        not resource_uri_matches(
            short_windows_resource,
            long_windows_resource.replace(
                "codestory://diagnostics/retrieval-engine",
                "codestory://status",
            ),
            platform_name="nt",
            samefile=same_windows_resource,
        )
        and len(identity_probes) == 1,
        "Windows project resource matching accepted a different resource base",
    )
    require(
        not resource_uri_matches(
            short_windows_resource,
            long_windows_resource.replace("%3A", "%3a"),
            platform_name="nt",
            samefile=same_windows_resource,
        )
        and len(identity_probes) == 1,
        "Windows project resource matching accepted noncanonical URI encoding",
    )
    require(
        not resource_uri_matches(
            short_windows_resource,
            long_windows_resource.replace("C%3A%2F", "relative%2F"),
            platform_name="nt",
            samefile=same_windows_resource,
        )
        and len(identity_probes) == 1,
        "Windows project resource matching accepted a relative project selector",
    )
    require(
        not resource_uri_matches(
            short_windows_resource,
            long_windows_resource,
            platform_name="nt",
            samefile=lambda _left, _right: False,
        ),
        "Windows project resource matching accepted a different native identity",
    )

    def missing_windows_resource(_left: Path, _right: Path) -> bool:
        raise FileNotFoundError("missing project resource")

    require(
        not resource_uri_matches(
            short_windows_resource,
            long_windows_resource,
            platform_name="nt",
            samefile=missing_windows_resource,
        ),
        "Windows project resource matching ignored an identity probe failure",
    )

    def fail_parallel(message: str) -> None:
        raise ProofFailure(message)

    try:
        run_parallel(
            {
                "z-task": lambda: fail_parallel("z failed"),
                "a-task": lambda: fail_parallel("a failed"),
            }
        )
    except ProofFailure as error:
        require(
            str(error)
            == "parallel qualification tasks failed: a-task: a failed; z-task: z failed",
            "parallel qualification failure aggregation is unstable",
        )
    else:
        raise ProofFailure("parallel qualification failures were ignored")

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

    require(
        require_native_process_start_identity(
            "linux:1234", "linux", "Linux self-test identity"
        )
        == "linux:1234"
        and require_native_process_start_identity(
            "macos-proc:1234:5678", "macos", "macOS self-test identity"
        )
        == "macos-proc:1234:5678"
        and require_native_process_start_identity(
            "windows:504911232000000010",
            "windows",
            "Windows self-test identity",
        )
        == "windows:504911232000000010",
        "canonical process identity format self-test failed",
    )
    for target_os, hostile_identity in (
        ("linux", "boot-id:1234"),
        ("macos", "Thu Jul 17 12:00:00 2026"),
        ("windows", "2026-07-17T12:00:00Z"),
    ):
        try:
            require_native_process_start_identity(
                hostile_identity,
                target_os,
                f"hostile {target_os} identity",
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"noncanonical {target_os} process identity format was accepted"
            )
