"""MCP installation and readiness self-tests."""

from __future__ import annotations

import json

from .foundation import REPOSITORY_ROOT, ProofFailure, require
from .subprocess_control import McpProcess


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


def _ready_retrieval_fixture() -> dict:

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

    return ready_retrieval


def _readiness_convergence_test(query: str, ready_retrieval: dict) -> None:
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


def _terminal_unavailable_test(query: str) -> None:
    unavailable = ScriptedMcpProcess(
        [
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
        ]
    )
    try:
        unavailable.search_until_ready({"query": query}, "self-test-unavailable")
    except ProofFailure as exc:
        require(
            "codestory_unavailable" in str(exc),
            f"terminal MCP failure omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("terminal MCP unavailable response was retried or accepted")
    require(
        len(unavailable.calls) == 1, "terminal MCP unavailable response was retried"
    )


def _hostile_result_tests(query: str, ready_retrieval: dict) -> None:
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


def run_installation_self_tests() -> None:
    query = "scripted-search"
    ready_retrieval = _ready_retrieval_fixture()
    _readiness_convergence_test(query, ready_retrieval)
    _terminal_unavailable_test(query)
    _hostile_result_tests(query, ready_retrieval)
