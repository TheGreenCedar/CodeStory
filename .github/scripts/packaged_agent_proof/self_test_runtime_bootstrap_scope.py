"""Proof-tier scope tests for the packaged runtime bootstrap."""

from __future__ import annotations

import argparse
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, call, patch

from . import runtime_bootstrap_continuity
from .foundation import ProofFailure, require

_PROJECT_A = Path("/self-test/large-project")
_PROJECT_B = Path("/self-test/small-project")
_QUESTION_A = "How does the large project activate?"
_QUERY_A = "large_project_probe"
_QUERY_B = "small_project_probe"
_CALIBRATION_LIVE_QUERY = (
    "small_project_probe calibration live encode verification"
)
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
) -> tuple[Mock, dict]:
    setup = _setup()
    cold = _cold()
    args = _args(proof_tier)
    host_a = Mock()
    host_b = Mock()
    cache_state = {
        "successful_encode_count": cold.snapshot_a["engine"][
            "successful_encode_count"
        ],
        "seen_queries": {_QUERY_B},
    }

    def advance_for_packet(*_arguments: object) -> tuple[dict, int]:
        cache_state["successful_encode_count"] += 1
        return {"self_test": "packet"}, 1

    if trap_project_a:
        host_a.tool_until_ready.side_effect = ProofFailure(
            "calibration scheduled a second broad project-A request"
        )
    else:
        host_a.tool_until_ready.side_effect = advance_for_packet
    host_a.search_until_ready.side_effect = ProofFailure(
        f"{proof_tier} moved its broad project-A request from packet to search"
    )

    def search_project_b(arguments: dict, _label: str) -> tuple[dict, int]:
        query = arguments["query"]
        if query not in cache_state["seen_queries"]:
            cache_state["seen_queries"].add(query)
            cache_state["successful_encode_count"] += 1
        return {"self_test": "search"}, 1

    def engine_diagnostics(*_arguments: object) -> dict:
        return {
            "engine": {
                "successful_encode_count": cache_state[
                    "successful_encode_count"
                ]
            },
            "process": {"server_instance_id": "server-self-test"},
        }

    host_b.search_until_ready.side_effect = search_project_b
    host_b.engine_diagnostics.side_effect = engine_diagnostics
    hosts = SimpleNamespace(
        host_a=host_a,
        host_b=host_b,
        start_a="host-a-start",
        start_b="host-b-start",
    )
    memory = {"self_test": "five-process-memory"}
    with (
        patch.object(
            runtime_bootstrap_continuity,
            "server_snapshot",
            side_effect=lambda diagnostics, _manifest, *, require_resident: diagnostics,
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
    expected_query = (
        _CALIBRATION_LIVE_QUERY
        if proof_tier == "calibration"
        else _QUERY_B
    )
    if proof_tier == "calibration":
        require(
            expected_query.strip().lower() != _QUERY_B.strip().lower(),
            "calibration live query reused the cold-phase embedding cache key",
        )
    require(
        host_b.method_calls
        == [
            call.search_until_ready(
                {
                    "project": str(_PROJECT_B),
                    "query": expected_query,
                    "why": True,
                },
                "search-b-live",
            ),
            call.engine_diagnostics(_PROJECT_B, "diagnostics-after-live"),
        ],
        f"{proof_tier} live retrieval changed the bounded project-B path",
    )
    after = memory_check.call_args.kwargs["snapshot"]
    require(
        after["engine"]["successful_encode_count"] == 8,
        f"{proof_tier} live retrieval did not model one fresh native encode",
    )
    snapshot_check.assert_called_once_with(
        after,
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
    return host_a, cache_state


def _live_retrieval_scope_tests() -> None:
    host_a, cache_state = _run_live_retrieval_case(
        "calibration",
        trap_project_a=True,
    )
    require(
        host_a.method_calls == [],
        "calibration scheduled a second broad operation on project A",
    )
    require(
        cache_state["seen_queries"] == {_QUERY_B, _CALIBRATION_LIVE_QUERY},
        "calibration did not prove a cache-distinct project-B query",
    )
    for proof_tier in ("hosted_package", "protected_hardware", "installed_runtime"):
        host_a, cache_state = _run_live_retrieval_case(proof_tier)
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
        require(
            cache_state["seen_queries"] == {_QUERY_B},
            f"{proof_tier} changed the established project-B query",
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
                    "why": True,
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
                    "why": True,
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
    _continuity_scope_tests()
