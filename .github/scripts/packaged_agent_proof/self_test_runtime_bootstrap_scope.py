"""Proof-tier scope tests for the packaged runtime bootstrap."""

from __future__ import annotations

import argparse
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, call, patch

from . import runtime_bootstrap_cold, runtime_bootstrap_continuity
from .foundation import ProofFailure, project_node_resource_uri, require

_PROJECT_A = Path("/self-test/large-project")
_PROJECT_B = Path("/self-test/small-project")
_QUESTION_A = "How does the large project activate?"
_QUERY_A = "large_project_probe"
_QUERY_B = "small_project_probe"
_MANIFEST = {"asset_target": "linux-x64"}


def _setup() -> SimpleNamespace:
    return SimpleNamespace(
        project_a=_PROJECT_A,
        project_b=_PROJECT_B,
        query_b=_QUERY_B,
        node=Path("/self-test/node"),
        command=["self-test-plugin-host"],
        qualified_env={"SELF_TEST": "runtime-bootstrap"},
    )


def _cold() -> SimpleNamespace:
    return SimpleNamespace(
        snapshot_a={"engine": {"successful_encode_count": 7}},
        shared_identity={"server_instance_id": "server-self-test"},
        status_a={"self_test": "status-a"},
        status_b={"self_test": "status-b"},
        identity_a={"embedding_backend": "CPU"},
    )


def _args(proof_tier: str) -> argparse.Namespace:
    return argparse.Namespace(
        proof_tier=proof_tier,
        produce_qualification_evidence=True,
        question=_QUESTION_A,
        query=_QUERY_A,
        timeout_secs=10,
        engine_policy="cpu_explicit",
        expected_backend="CPU",
    )


def _run_live_retrieval_case(
    proof_tier: str,
    *,
    trap_project_a: bool = False,
) -> Mock:
    setup = _setup()
    cold = _cold()
    args = _args(proof_tier)
    host_a = Mock()
    host_b = Mock()
    if trap_project_a:
        host_a.tool_until_ready.side_effect = ProofFailure(
            "calibration scheduled a second broad project-A request"
        )
    else:
        host_a.tool_until_ready.return_value = ({"self_test": "packet"}, 1)
    host_a.search_until_ready.side_effect = ProofFailure(
        f"{proof_tier} moved its broad project-A request from packet to search"
    )
    host_b.search_until_ready.return_value = ({"self_test": "search"}, 1)
    host_b.engine_diagnostics.return_value = {"self_test": "diagnostics"}
    hosts = SimpleNamespace(
        host_a=host_a,
        host_b=host_b,
        start_a="host-a-start",
        start_b="host-b-start",
    )
    after = {
        "engine": {"successful_encode_count": 8},
        "process": {"server_instance_id": "server-self-test"},
    }
    memory = {"self_test": "five-process-memory"}
    with (
        patch.object(
            runtime_bootstrap_continuity,
            "server_snapshot",
            return_value=after,
        ) as snapshot_check,
        patch.object(
            runtime_bootstrap_continuity,
            "capture_five_process_memory",
            return_value=memory,
        ) as memory_check,
    ):
        observed = runtime_bootstrap_continuity._live_retrieval(
            args,
            setup,
            hosts,
            cold,
            _MANIFEST,
        )
    require(
        observed == memory,
        f"{proof_tier} live retrieval omitted five-process memory evidence",
    )
    require(
        host_b.method_calls
        == [
            call.search_until_ready(
                {
                    "project": str(_PROJECT_B),
                    "query": _QUERY_B,
                },
                "search-b-live",
            ),
            call.engine_diagnostics(_PROJECT_B, "diagnostics-after-live"),
        ],
        f"{proof_tier} live retrieval changed the bounded project-B path",
    )
    snapshot_check.assert_called_once_with(
        host_b.engine_diagnostics.return_value,
        _MANIFEST,
        require_resident=True,
    )
    memory_check.assert_called_once_with(
        args=args,
        node_path=setup.node,
        host_a=host_a,
        host_a_start=hosts.start_a,
        host_b=host_b,
        host_b_start=hosts.start_b,
        status_a=cold.status_a,
        status_b=cold.status_b,
        snapshot=after,
        manifest=_MANIFEST,
        expected_backend="CPU",
    )
    return host_a


def _live_retrieval_scope_tests() -> None:
    host_a = _run_live_retrieval_case("calibration", trap_project_a=True)
    require(
        host_a.method_calls == [],
        "calibration scheduled a second broad operation on project A",
    )
    for proof_tier in ("hosted_package", "protected_hardware", "installed_runtime"):
        host_a = _run_live_retrieval_case(proof_tier)
        require(
            host_a.method_calls
            == [
                call.tool_until_ready(
                    "packet",
                    {
                        "project": str(_PROJECT_A),
                        "question": _QUESTION_A,
                        "budget": "compact",
                    },
                    "packet-a",
                )
            ],
            f"{proof_tier} no longer requires the project-A packet path",
        )


def _v3_snippet_contract_test() -> None:
    setup = _setup()
    node_id = "node-v3-search-evidence"
    search = {
        "kind": "complete",
        "schema_version": 3,
        "evidence": [
            {"path": "lib.rs", "symbol_id": None},
            {"path": "lib.rs", "symbol_id": node_id},
        ],
    }
    cold = SimpleNamespace(
        results={
            "search-b": (
                {"result": {"structuredContent": search, "isError": False}},
                1,
            )
        }
    )
    snippet = {
        "scope": "function_body",
        "requested_context": 0,
        "range_source": "callable",
        "snippet": f"pub fn {_QUERY_B}() {{}}",
        "node": {"id": node_id},
    }
    host_b = Mock()
    host_b.resource.return_value = {"node": {"id": node_id}}
    host_b.tool_until_ready.return_value = (
        {"result": {"structuredContent": snippet, "isError": False}},
        2,
    )
    hosts = SimpleNamespace(host_b=host_b)

    observed, attempts = runtime_bootstrap_cold._snippet_contract(setup, hosts, cold)

    expected_uri = project_node_resource_uri(
        "codestory://snippet", node_id, _PROJECT_B
    )
    require(
        observed == snippet and attempts == 2,
        "v3 search evidence did not drive the packaged snippet contract",
    )
    require(
        host_b.method_calls
        == [
            call.resource(expected_uri, "snippet-resource-contract"),
            call.tool_until_ready(
                "snippet",
                {
                    "project": str(_PROJECT_B),
                    "id": node_id,
                    "function_body": True,
                    "lines": 0,
                },
                "snippet-contract",
            ),
        ],
        "packaged snippet did not use the exact v3 evidence symbol identity",
    )


def _run_continuity_case(proof_tier: str, expected_project: Path) -> None:
    setup = _setup()
    cold = _cold()
    args = _args(proof_tier)
    host_a = Mock()
    host_a.process.pid = 101
    host_b = Mock()
    host_b.process.pid = 202
    host_b.search_until_ready.return_value = ({"self_test": "survivor"}, 1)
    host_b.engine_diagnostics.return_value = {"self_test": "survivor-diagnostics"}
    hosts = SimpleNamespace(
        host_a=host_a,
        host_b=host_b,
        start_a="host-a-start",
        start_b="host-b-start",
    )
    host_c = Mock()
    host_c.process.pid = 303
    host_c.transcript = []

    def rejoin_search(arguments: dict, label: str) -> tuple[dict, int]:
        if (
            proof_tier == "calibration"
            and arguments.get("project") == str(_PROJECT_A)
        ):
            raise ProofFailure(
                "calibration rejoined through a broad project-A request"
            )
        return {"self_test": label}, 1

    host_c.search_until_ready.side_effect = rejoin_search
    host_c.engine_diagnostics.return_value = {"self_test": "rejoin-diagnostics"}
    survivor = {
        "engine": {"successful_encode_count": 9},
        "process": {"server_instance_id": "server-self-test"},
    }
    rejoin = {
        "engine": {"successful_encode_count": 10},
        "process": {"server_instance_id": "server-self-test"},
    }
    rejoin_identity = {"embedding_materialized_reused": True}
    with (
        tempfile.TemporaryDirectory() as temporary,
        patch.object(
            runtime_bootstrap_continuity,
            "McpProcess",
            return_value=host_c,
        ) as process_constructor,
        patch.object(
            runtime_bootstrap_continuity,
            "process_start_identity",
            return_value="host-c-start",
        ) as start_identity,
        patch.object(
            runtime_bootstrap_continuity,
            "server_snapshot",
            side_effect=[survivor, rejoin],
        ) as snapshot_check,
        patch.object(
            runtime_bootstrap_continuity,
            "engine_identity",
            return_value=rejoin_identity,
        ) as identity_check,
    ):
        observed = runtime_bootstrap_continuity._continuity_proof(
            args,
            setup,
            hosts,
            cold,
            _MANIFEST,
            Path(temporary),
        )
    require(
        observed.survivor == survivor
        and observed.rejoin_snapshot == rejoin
        and observed.rejoin_identity == rejoin_identity,
        f"{proof_tier} continuity evidence changed",
    )
    require(
        host_a.method_calls == [call.kill()],
        f"{proof_tier} continuity did not replace exactly host A",
    )
    require(
        host_b.method_calls
        == [
            call.search_until_ready(
                {
                    "project": str(_PROJECT_B),
                    "query": _QUERY_B,
                },
                "survivor-search",
            ),
            call.engine_diagnostics(_PROJECT_B, "survivor-diagnostics"),
        ],
        f"{proof_tier} continuity changed the surviving project-B path",
    )
    process_constructor.assert_called_once_with(
        setup.command,
        env=setup.qualified_env,
        cwd=expected_project,
        timeout=args.timeout_secs,
    )
    start_identity.assert_called_once_with(host_c.process.pid)
    require(
        host_c.method_calls
        == [
            call.initialize(),
            call.search_until_ready(
                {
                    "project": str(expected_project),
                    "query": _QUERY_B if expected_project == _PROJECT_B else _QUERY_A,
                },
                "rejoin-search",
            ),
            call.engine_diagnostics(expected_project, "rejoin-diagnostics"),
            call.close(),
        ],
        f"{proof_tier} continuity changed its replacement-host scope",
    )
    require(
        snapshot_check.call_args_list
        == [
            call(
                host_b.engine_diagnostics.return_value,
                _MANIFEST,
                require_resident=True,
            ),
            call(
                host_c.engine_diagnostics.return_value,
                _MANIFEST,
                require_resident=True,
            ),
        ],
        f"{proof_tier} continuity changed its resident snapshot checks",
    )
    identity_check.assert_called_once_with(
        host_c.engine_diagnostics.return_value,
        args.engine_policy,
        args.expected_backend,
    )


def _continuity_scope_tests() -> None:
    _run_continuity_case("calibration", _PROJECT_B)
    for proof_tier in ("hosted_package", "protected_hardware", "installed_runtime"):
        _run_continuity_case(proof_tier, _PROJECT_A)


def run_runtime_bootstrap_scope_self_tests() -> None:
    _live_retrieval_scope_tests()
    _v3_snippet_contract_test()
    _continuity_scope_tests()
