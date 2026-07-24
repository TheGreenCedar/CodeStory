"""Self Test for packaged CodeStory proof."""

from __future__ import annotations

import json

from .foundation import ProofFailure, REPOSITORY_ROOT, require
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

def run_installation_self_tests() -> None:
    class ScriptedMcpProcess(McpProcess):
        def __init__(self, responses: list[dict]):
            self.timeout = 1
            self.responses = iter(responses)
            self.calls: list[tuple[str, dict, str]] = []
            self.tool_attempt_counts: dict[str, int] = {}

        def tool(self, name: str, arguments: dict, request_id: str) -> dict:
            self.calls.append((name, arguments, request_id))
            try:
                return next(self.responses)
            except StopIteration as exc:
                raise ProofFailure("scripted MCP response sequence was exhausted") from exc

    projection_fixture_path = (
        REPOSITORY_ROOT
        / "crates"
        / "codestory-cli"
        / "tests"
        / "fixtures"
        / "stdio_installed_host_search_retrieval.json"
    )
    projection_fixture = json.loads(projection_fixture_path.read_text(encoding="utf-8"))
    require(
        isinstance(projection_fixture, dict),
        f"installed search projection fixture is not an object: {projection_fixture!r}",
    )
    ready_retrieval = projection_fixture.get("projected")
    require(
        isinstance(ready_retrieval, dict),
        f"installed search projection fixture is missing projected retrieval: {projection_fixture!r}",
    )

    query = "scripted-search"
    preparing = {
        "result": {
            "isError": True,
            "structuredContent": {
                "code": "codestory_preparing",
                "state": "preparing",
                "retry_tool": "search",
                "retry_after_ms": 0,
            },
        }
    }
    ready = {
        "result": {
            "structuredContent": {
                "query": query,
                "hits": [],
                "retrieval": ready_retrieval,
            }
        }
    }
    scripted = ScriptedMcpProcess([preparing, ready])
    _, attempts = scripted.search_until_ready({"query": query}, "self-test-search")
    require(attempts == 2, "preparing search did not converge on its second attempt")
    require(
        scripted.tool_attempt_counts.get("self-test-search") == 2,
        "preparing search attempt count was not retained",
    )

    unavailable = ScriptedMcpProcess([
        {
            "result": {
                "isError": True,
                "structuredContent": {
                    "code": "codestory_unavailable",
                    "state": "unavailable",
                    "message": "hostile terminal response",
                },
            }
        }
    ])
    try:
        unavailable.search_until_ready({"query": query}, "self-test-unavailable")
    except ProofFailure as exc:
        require(
            "codestory_unavailable" in str(exc),
            f"terminal MCP failure omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("terminal MCP unavailable response was retried or accepted")
    require(len(unavailable.calls) == 1, "terminal MCP unavailable response was retried")

    hostile_search_results = [
        (
            "legacy mode=full",
            {"query": query, "hits": [], "retrieval": {"mode": "full"}},
            "ready installed retrieval projection",
        ),
        (
            "preparing retrieval projection",
            {"query": query, "hits": [], "retrieval": {"state": "preparing"}},
            "ready installed retrieval projection",
        ),
        (
            "missing retrieval projection",
            {"query": query, "hits": []},
            "ready installed retrieval projection",
        ),
        (
            "non-array hits",
            {"query": query, "hits": {}, "retrieval": ready_retrieval},
            "non-array hits",
        ),
    ]
    for label, structured_content, expected_diagnostic in hostile_search_results:
        hostile = ScriptedMcpProcess(
            [{"result": {"structuredContent": structured_content}}]
        )
        try:
            hostile.search_until_ready({"query": query}, f"self-test-{label}")
        except ProofFailure as exc:
            require(
                expected_diagnostic in str(exc),
                f"{label} failure omitted its diagnostics: {exc}",
            )
        else:
            raise ProofFailure(f"{label} search result was accepted")
        require(len(hostile.calls) == 1, f"{label} search result was retried")
