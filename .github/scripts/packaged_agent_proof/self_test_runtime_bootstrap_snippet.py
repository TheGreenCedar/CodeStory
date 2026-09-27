"""Hermetic snippet-contract tests for legacy hits and schema-3 evidence."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, call

from .foundation import ProofFailure, project_node_resource_uri, require
from .runtime_bootstrap_cold import _snippet_contract
from .subprocess_control import resolve_search_snippet_anchor

_PROJECT_B = Path("/self-test/small-project")
_QUERY_B = "shared_engine_probe"
_NODE_ID = "node-self-test-1"


def _legacy_search(*, include_snippet_link: bool = True) -> dict:
    links = [{"rel": "definition", "uri": "codestory://symbol/ignored"}]
    if include_snippet_link:
        links.append(
            {
                "rel": "snippet",
                "uri": project_node_resource_uri(
                    "codestory://snippet", _NODE_ID, _PROJECT_B
                ),
            }
        )
    return {
        "query": _QUERY_B,
        "hits": [
            {
                "node_id": _NODE_ID,
                "links": links,
            }
        ],
        "retrieval": {"state": "ready"},
    }


def _schema3_search(*, symbol_id: object = _NODE_ID) -> dict:
    return {
        "kind": "complete",
        "schema_version": 3,
        "status": "available",
        "evidence": [
            {
                "identity": {"evidence_id": "evidence-1"},
                "path": "src/probe.rs",
                "symbol_id": None,
                "start_line": 1,
                "end_line": 1,
                "excerpt": "fn other() {}",
            },
            {
                "identity": {"evidence_id": "evidence-2"},
                "path": "src/shared_engine_probe.rs",
                "symbol_id": symbol_id,
                "start_line": 4,
                "end_line": 8,
                "excerpt": f"fn {_QUERY_B}() {{}}",
            },
        ],
        "retrieval": {"state": "full", "generation_id": "retrieval-self-test"},
        "answer_sufficiency": "not_asserted",
    }


def _legacy_hits_anchor_test() -> None:
    anchor = resolve_search_snippet_anchor(_legacy_search())
    require(
        anchor
        == {
            "node_id": _NODE_ID,
            "snippet_link_uri": project_node_resource_uri(
                "codestory://snippet", _NODE_ID, _PROJECT_B
            ),
        },
        f"legacy hits snippet anchor drifted: {anchor!r}",
    )


def _schema3_evidence_anchor_test() -> None:
    search = _schema3_search()
    require(
        "hits" not in search and "query" not in search,
        "schema-3 fixture accidentally included legacy query/hits fields",
    )
    anchor = resolve_search_snippet_anchor(search)
    require(
        anchor == {"node_id": _NODE_ID, "snippet_link_uri": None},
        f"schema-3 evidence snippet anchor drifted: {anchor!r}",
    )


def _hostile_anchor_tests() -> None:
    hostile_cases = [
        (
            "legacy missing hits",
            {"query": _QUERY_B, "retrieval": {"state": "ready"}},
            "non-array hits",
        ),
        (
            "legacy non-array hits",
            {"query": _QUERY_B, "hits": {}, "retrieval": {"state": "ready"}},
            "non-array hits",
        ),
        (
            "legacy hits without snippet link",
            _legacy_search(include_snippet_link=False),
            "resolvable hit with continuation links",
        ),
        (
            "schema3 incomplete kind",
            {**_schema3_search(), "kind": "budget_exceeded"},
            "complete schema-3 evidence projection",
        ),
        (
            "schema3 non-array evidence",
            {**_schema3_search(), "evidence": {}},
            "non-array evidence",
        ),
        (
            "schema3 evidence without symbol_id",
            _schema3_search(symbol_id=None),
            "resolvable schema-3 evidence with symbol_id",
        ),
        (
            "schema3 empty symbol_id",
            _schema3_search(symbol_id=""),
            "resolvable schema-3 evidence with symbol_id",
        ),
    ]
    for label, payload, expected_diagnostic in hostile_cases:
        try:
            resolve_search_snippet_anchor(payload)
        except ProofFailure as exc:
            require(
                expected_diagnostic in str(exc),
                f"{label} failure omitted its diagnostics: {exc}",
            )
        else:
            raise ProofFailure(f"{label} search projection was accepted as a snippet anchor")


def _run_snippet_contract(search: dict) -> tuple[dict, int, Mock]:
    expected_uri = project_node_resource_uri(
        "codestory://snippet", _NODE_ID, _PROJECT_B
    )
    host_b = Mock()
    host_b.resource.return_value = {"node": {"id": _NODE_ID}}
    host_b.tool_until_ready.return_value = (
        {
            "result": {
                "structuredContent": {
                    "scope": "function_body",
                    "requested_context": 0,
                    "range_source": "definition",
                    "snippet": f"fn {_QUERY_B}() {{ return 1; }}",
                    "node": {"id": _NODE_ID},
                }
            }
        },
        1,
    )
    setup = SimpleNamespace(project_b=_PROJECT_B, query_b=_QUERY_B)
    hosts = SimpleNamespace(host_b=host_b)
    cold = SimpleNamespace(
        results={
            "search-b": (
                {"result": {"structuredContent": search}},
                1,
            )
        }
    )
    snippet, attempts = _snippet_contract(setup, hosts, cold)
    require(attempts == 1, "snippet contract retained the wrong attempt count")
    require(
        snippet["node"]["id"] == _NODE_ID,
        f"snippet contract lost the linked node identity: {snippet!r}",
    )
    require(
        host_b.method_calls
        == [
            call.resource(expected_uri, "snippet-resource-contract"),
            call.tool_until_ready(
                "snippet",
                {
                    "project": str(_PROJECT_B),
                    "id": _NODE_ID,
                    "function_body": True,
                    "lines": 0,
                },
                "snippet-contract",
            ),
        ],
        f"snippet contract MCP calls drifted: {host_b.method_calls!r}",
    )
    return snippet, attempts, host_b


def _legacy_hits_snippet_contract_test() -> None:
    _run_snippet_contract(_legacy_search())


def _schema3_evidence_snippet_contract_test() -> None:
    _run_snippet_contract(_schema3_search())


def run_runtime_bootstrap_snippet_self_tests() -> None:
    _legacy_hits_anchor_test()
    _schema3_evidence_anchor_test()
    _hostile_anchor_tests()
    _legacy_hits_snippet_contract_test()
    _schema3_evidence_snippet_contract_test()
