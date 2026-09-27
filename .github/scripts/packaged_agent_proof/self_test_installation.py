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

    def tool(
        self,
        name: str,
        arguments: dict,
        request_id: str,
        deadline: float | None = None,
    ) -> dict:
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


def _text_preparing_2024_11_05_convergence_test(
    query: str, ready_retrieval: dict
) -> None:
    # Live MCP 2024-11-05 fail-open preparing omits structuredContent and puts the
    # kind/state envelope in content[0].text. Qual Metal/Windows proofs rejected that
    # shape before retrying; keep the structured code path covered above and prove the
    # text envelope retries equivalently here.
    preparing_envelope = {
        "kind": "preparing",
        "state": "preparing",
        "retry_after_ms": 0,
        "minimum_next": {"kind": "retry_same_request", "after_ms": 0},
        "operation": {
            "progress": 20,
            "stage": "core_freshness",
            "state": "updating",
        },
    }
    preparing = {
        "result": {
            "isError": False,
            "content": [
                {"type": "text", "text": json.dumps(preparing_envelope)},
            ],
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
    response, attempts = scripted.search_until_ready(
        {"query": query}, "self-test-text-preparing"
    )
    require(
        attempts == 2,
        "2024-11-05 text preparing search did not converge on its second attempt",
    )
    require(
        scripted.tool_attempt_counts.get("self-test-text-preparing") == 2,
        "2024-11-05 text preparing attempt count was not retained",
    )
    require(
        response["result"]["structuredContent"]["retrieval"] == ready_retrieval,
        "2024-11-05 text preparing convergence lost the ready structured payload",
    )

    ground_preparing = {
        "result": {
            "isError": False,
            "content": [
                {"type": "text", "text": json.dumps(preparing_envelope)},
            ],
        }
    }
    ground_ready_payload = {
        "project": "/self-test",
        "state": "ready",
        "budget": "strict",
    }
    ground_ready = {
        "result": {
            "isError": False,
            "content": [
                {"type": "text", "text": json.dumps(ground_ready_payload)},
            ],
        }
    }
    ground_host = ScriptedMcpProcess([ground_preparing, ground_ready])
    ground_response, ground_attempts = ground_host.tool_until_ready(
        "ground",
        {"project": "/self-test", "budget": "strict"},
        "self-test-text-ground",
    )
    require(
        ground_attempts == 2,
        "2024-11-05 text preparing ground did not converge on its second attempt",
    )
    require(
        ground_response["result"]["structuredContent"] == ground_ready_payload,
        "2024-11-05 text-only ready ground was not exposed as structuredContent",
    )

    # Terminal unavailable must still fail closed even when delivered as text only.
    unavailable_envelope = {
        "code": "codestory_unavailable",
        "state": "unavailable",
        "message": "hostile terminal response",
    }
    unavailable = ScriptedMcpProcess(
        [
            {
                "result": {
                    "isError": True,
                    "content": [
                        {
                            "type": "text",
                            "text": json.dumps(unavailable_envelope),
                        }
                    ],
                }
            }
        ]
    )
    try:
        unavailable.tool_until_ready(
            "ground",
            {"project": "/self-test", "budget": "strict"},
            "self-test-text-unavailable",
        )
    except ProofFailure as exc:
        require(
            "codestory_unavailable" in str(exc),
            f"text-only terminal MCP failure omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("text-only terminal MCP unavailable response was retried")
    require(
        len(unavailable.calls) == 1,
        "text-only terminal MCP unavailable response was retried",
    )


def _degraded_convergence_test(query: str, ready_retrieval: dict) -> None:
    # The truthful projection answers lexically while the semantic sidecar is
    # still publishing; that window must read as convergence, not failure.
    degraded = {
        "result": {
            "structuredContent": {
                "query": query,
                "hits": [],
                "retrieval": {
                    "state": "degraded",
                    "mode": "lexical",
                    "fallback_reason": "semantic_unpublished",
                },
            }
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
    scripted = ScriptedMcpProcess([degraded, ready])
    _, attempts = scripted.search_until_ready({"query": query}, "self-test-degraded")
    require(attempts == 2, "degraded search did not converge on its second poll")
    require(
        scripted.tool_attempt_counts.get("self-test-degraded") == 2,
        "degraded search attempt count was not retained",
    )

    # A host that never converges must fail loud at the deadline, never hang
    # and never pass on a degraded answer.
    stuck = ScriptedMcpProcess([degraded] * 8)
    try:
        stuck.search_until_ready({"query": query}, "self-test-degraded-stuck")
    except ProofFailure as exc:
        require(
            "never became ready" in str(exc),
            f"stuck degraded projection omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("a projection stuck degraded was accepted as ready")


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


def _schema3_complete_envelope(
    *,
    retrieval_state: str = "full",
    evidence: list | None = None,
    kind: str = "complete",
    generation_id: str | None = "retrieval-self-test",
) -> dict:
    return {
        "kind": kind,
        "schema_version": 3,
        "status": "available",
        "evidence": [] if evidence is None else evidence,
        "retrieval": {
            "state": retrieval_state,
            "generation_id": generation_id,
        },
        "answer_sufficiency": "not_asserted",
    }


def _schema3_convergence_test(query: str) -> None:
    # Live MCP search returns the schema-3 evidence projection (no query/hits echo).
    # Qual Metal search-a/search-b failed when the harness still required legacy fields.
    preparing = {
        "result": {
            "structuredContent": {
                "code": "codestory_preparing",
                "state": "preparing",
                "retry_tool": "search",
                "retry_after_ms": 0,
            },
        }
    }
    ready_payload = _schema3_complete_envelope(
        evidence=[
            {
                "identity": {"evidence_id": "evidence-1"},
                "kind": "exact_source",
                "path": "src/lib.rs",
                "summary": "self-test hit",
            }
        ]
    )
    ready = {"result": {"structuredContent": ready_payload}}
    scripted = ScriptedMcpProcess([preparing, ready])
    response, attempts = scripted.search_until_ready(
        {"query": query}, "self-test-schema3-search"
    )
    require(
        attempts == 2,
        "schema-3 preparing search did not converge on its second attempt",
    )
    require(
        scripted.tool_attempt_counts.get("self-test-schema3-search") == 2,
        "schema-3 preparing search attempt count was not retained",
    )
    require(
        response["result"]["structuredContent"] == ready_payload,
        "schema-3 structured search convergence lost the evidence projection",
    )
    require(
        "query" not in ready_payload and "hits" not in ready_payload,
        "schema-3 self-test fixture accidentally included legacy query/hits fields",
    )


def _schema3_text_only_convergence_test(query: str) -> None:
    # Protocol 2024-11-05 may deliver the complete evidence envelope as text only.
    # tool_until_ready already attaches it as structuredContent; search_until_ready
    # must accept that attached schema-3 shape the same way.
    preparing_envelope = {
        "kind": "preparing",
        "state": "preparing",
        "retry_after_ms": 0,
        "minimum_next": {"kind": "retry_same_request", "after_ms": 0},
    }
    preparing = {
        "result": {
            "isError": False,
            "content": [{"type": "text", "text": json.dumps(preparing_envelope)}],
        }
    }
    ready_payload = _schema3_complete_envelope()
    ready = {
        "result": {
            "isError": False,
            "content": [{"type": "text", "text": json.dumps(ready_payload)}],
        }
    }
    scripted = ScriptedMcpProcess([preparing, ready])
    response, attempts = scripted.search_until_ready(
        {"query": query}, "self-test-schema3-text"
    )
    require(
        attempts == 2,
        "schema-3 text-only search did not converge on its second attempt",
    )
    require(
        response["result"]["structuredContent"] == ready_payload,
        "schema-3 text-only ready search was not exposed as structuredContent",
    )


def _schema3_degraded_convergence_test(query: str) -> None:
    degraded = {
        "result": {
            "structuredContent": _schema3_complete_envelope(retrieval_state="degraded")
        }
    }
    ready = {
        "result": {
            "structuredContent": _schema3_complete_envelope(retrieval_state="full")
        }
    }
    scripted = ScriptedMcpProcess([degraded, ready])
    _, attempts = scripted.search_until_ready(
        {"query": query}, "self-test-schema3-degraded"
    )
    require(
        attempts == 2,
        "schema-3 degraded search did not converge on its second poll",
    )

    stuck = ScriptedMcpProcess([degraded] * 8)
    try:
        stuck.search_until_ready({"query": query}, "self-test-schema3-degraded-stuck")
    except ProofFailure as exc:
        require(
            "never became ready" in str(exc),
            f"stuck schema-3 degraded projection omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("a schema-3 projection stuck degraded was accepted as ready")


def _schema3_hostile_result_tests(query: str) -> None:
    hostile_search_results = [
        (
            "schema3 retrieval unavailable",
            _schema3_complete_envelope(retrieval_state="unavailable"),
            "ready installed retrieval projection",
        ),
        (
            "schema3 retrieval symbolic",
            _schema3_complete_envelope(retrieval_state="symbolic"),
            "ready installed retrieval projection",
        ),
        (
            "schema3 budget exceeded",
            _schema3_complete_envelope(kind="budget_exceeded", retrieval_state="full"),
            "complete schema-3 evidence projection",
        ),
        (
            "schema3 non-array evidence",
            {
                **_schema3_complete_envelope(),
                "evidence": {},
            },
            "non-array evidence",
        ),
        (
            "schema3 legacy ready state name",
            _schema3_complete_envelope(retrieval_state="ready"),
            "ready installed retrieval projection",
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
    _text_preparing_2024_11_05_convergence_test(query, ready_retrieval)
    _schema3_convergence_test(query)
    _schema3_text_only_convergence_test(query)
    _schema3_degraded_convergence_test(query)
    _schema3_hostile_result_tests(query)
    _degraded_convergence_test(query, ready_retrieval)
    _terminal_unavailable_test(query)
    _hostile_result_tests(query, ready_retrieval)
