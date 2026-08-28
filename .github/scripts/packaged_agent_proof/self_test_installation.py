"""MCP installation and readiness self-tests."""

from __future__ import annotations

from copy import deepcopy
import hashlib
import json
from types import SimpleNamespace

from .foundation import ProofFailure, require
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


class ScriptedInitializeProcess(McpProcess):
    def __init__(self, response_protocol: str):
        self.response_protocol = response_protocol
        self.requests: list[dict] = []
        self.notifications: list[dict] = []
        self.process = SimpleNamespace(stdin=self)

    def send(self, request: dict, deadline: float | None = None) -> dict:
        del deadline
        self.requests.append(request)
        return {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "result": {"protocolVersion": self.response_protocol},
        }

    def write(self, payload: str) -> None:
        self.notifications.append(json.loads(payload))

    def flush(self) -> None:
        return None


def _modern_protocol_initialization_test() -> None:
    modern = ScriptedInitializeProcess("2025-11-25")
    modern.initialize()
    require(
        modern.requests[0]["params"]["protocolVersion"] == "2025-11-25",
        "packaged proof did not request CodeStory's preferred MCP revision",
    )
    require(
        modern.notifications
        == [{"jsonrpc": "2.0", "method": "notifications/initialized"}],
        "packaged proof did not complete the preferred-revision handshake",
    )

    downgraded = ScriptedInitializeProcess("2024-11-05")
    try:
        downgraded.initialize()
    except ProofFailure as exc:
        require(
            "protocol revision" in str(exc),
            f"protocol downgrade failure omitted its diagnostic: {exc}",
        )
    else:
        raise ProofFailure("packaged proof accepted a downgraded MCP revision")


def _search_projection(query: str, retrieval_state: str) -> dict:
    retrieval_generation = (
        "retrieval-self-test" if retrieval_state == "full" else None
    )
    return {
        "kind": "complete",
        "schema_version": 3,
        "identity": {
            "packet_id": "packet-self-test",
            "request_id": "request-self-test",
            "question_sha256": hashlib.sha256(query.encode("utf-8")).hexdigest(),
        },
        "publication": {
            "core": {
                "project_id": "project-self-test",
                "generation_id": "core-generation-self-test",
                "run_id": "core-run-self-test",
            },
            "retrieval": (
                {
                    "core_generation_id": "core-generation-self-test",
                    "core_run_id": "core-run-self-test",
                    "retrieval_generation": retrieval_generation,
                    "retrieval_input_sha256": "a" * 64,
                    "semantic_generation": "semantic-generation-self-test",
                }
                if retrieval_generation is not None
                else None
            ),
        },
        "status": "available",
        "evidence": [],
        "gaps": [],
        "continuation": None,
        "retrieval": {
            "state": retrieval_state,
            "generation_id": retrieval_generation,
        },
        "diagnostics": {"availability": "unavailable"},
    }


def _readiness_convergence_test(query: str) -> None:
    preparing = {
        "result": {
            "structuredContent": {
                "kind": "preparing",
                "state": "preparing",
                "retry_after_ms": 1,
                "operation": {"stage": "publication"},
            },
            "isError": False,
        }
    }
    ready = {
        "result": {
            "structuredContent": _search_projection(query, "full"),
            "isError": False,
        }
    }
    scripted = ScriptedMcpProcess([preparing, ready])
    _, attempts = scripted.search_until_ready({"query": query}, "self-test-search")
    require(attempts == 2, "preparing search did not converge on its second attempt")
    require(
        scripted.tool_attempt_counts.get("self-test-search") == 2,
        "preparing search attempt count was not retained",
    )


def _degraded_convergence_test(query: str) -> None:
    # The truthful projection answers lexically while the semantic sidecar is
    # still publishing; that window must read as convergence, not failure.
    degraded = {
        "result": {
            "structuredContent": _search_projection(query, "degraded"),
            "isError": False,
        }
    }
    ready = {
        "result": {
            "structuredContent": _search_projection(query, "full"),
            "isError": False,
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
                    "content": [
                        {
                            "type": "text",
                            "text": '{"code":"codestory_unavailable","message":"hostile terminal response"}',
                        }
                    ],
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


def _hostile_result_tests(query: str) -> None:
    wrong_hash = _search_projection(query, "full")
    wrong_hash["identity"]["question_sha256"] = "0" * 64
    non_array_evidence = _search_projection(query, "full")
    non_array_evidence["evidence"] = {}
    non_array_gaps = _search_projection(query, "full")
    non_array_gaps["gaps"] = {}
    old_retrieval_state = _search_projection(query, "full")
    old_retrieval_state["retrieval"]["state"] = "ready"
    missing_generation = _search_projection(query, "full")
    missing_generation["retrieval"]["generation_id"] = None
    missing_publication = _search_projection(query, "full")
    del missing_publication["publication"]
    mismatched_generation = _search_projection(query, "full")
    mismatched_generation["publication"]["retrieval"][
        "retrieval_generation"
    ] = "other-retrieval-generation"
    malformed_evidence = _search_projection(query, "full")
    malformed_evidence["evidence"] = [
        {
            "identity": {},
            "path": "lib.rs",
            "symbol_id": None,
            "start_line": 1,
            "end_line": 1,
            "excerpt": "self-test",
        }
    ]
    malformed_gap = _search_projection(query, "full")
    malformed_gap["gaps"] = [
        {
            "identity": {"gap_id": "gap-self-test"},
            "kind": "invented_gap",
            "message": None,
        }
    ]
    malformed_diagnostics = _search_projection(query, "full")
    malformed_diagnostics["diagnostics"] = {
        "availability": "available",
        "reference": {
            "artifact_id": "artifact-self-test",
            "sha256": "short",
            "byte_length": 1,
            "uri": "codestory://packet-diagnostics/self-test",
            "wall_expiry_epoch_ms": 1,
        },
    }
    malformed_continuation = _search_projection(query, "full")
    malformed_continuation["continuation"] = {
        "continuation_id": "continuation-self-test",
        "remaining_rounds": 0,
        "gap_ids": [],
    }
    malformed_status = _search_projection(query, "full")
    malformed_status["status"] = "ready"
    hostile_search_results = [
        (
            "legacy v2 search",
            {"query": query, "hits": [], "retrieval": {"state": "ready"}},
            "closed object shape",
        ),
        (
            "mismatched question hash",
            wrong_hash,
            "invalid v3 search identity",
        ),
        (
            "non-array evidence",
            non_array_evidence,
            "invalid v3 evidence collection",
        ),
        (
            "non-array gaps",
            non_array_gaps,
            "invalid v3 gap collection",
        ),
        (
            "old retrieval state",
            old_retrieval_state,
            "full or degraded v3 retrieval projection",
        ),
        (
            "full retrieval missing generation",
            missing_generation,
            "retrieval generation did not match",
        ),
        (
            "missing publication",
            missing_publication,
            "closed object shape",
        ),
        (
            "mismatched retrieval generation",
            mismatched_generation,
            "retrieval generation did not match",
        ),
        (
            "malformed evidence row",
            malformed_evidence,
            "closed object shape",
        ),
        (
            "malformed gap row",
            malformed_gap,
            "invalid v3 gap",
        ),
        (
            "malformed diagnostics",
            malformed_diagnostics,
            "invalid diagnostics reference",
        ),
        (
            "malformed continuation",
            malformed_continuation,
            "invalid v3 continuation",
        ),
        (
            "malformed status",
            malformed_status,
            "invalid v3 search projection",
        ),
    ]
    for label, structured_content, expected_diagnostic in hostile_search_results:
        hostile = ScriptedMcpProcess(
            [
                {
                    "result": {
                        "structuredContent": structured_content,
                        "isError": False,
                    }
                }
            ]
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

    valid_ready = _search_projection(query, "full")
    for malformed in (None, 1, "true"):
        result = {"structuredContent": valid_ready}
        if malformed is not None:
            result["isError"] = malformed
        hostile = ScriptedMcpProcess([{"result": result}])
        try:
            hostile.search_until_ready(
                {"query": query}, f"self-test-is-error-{malformed!r}"
            )
        except ProofFailure as exc:
            require(
                "isError" in str(exc),
                f"malformed isError failure omitted its diagnostic: {exc}",
            )
        else:
            raise ProofFailure(f"malformed isError={malformed!r} was accepted")

    preparing = {
        "kind": "preparing",
        "state": "preparing",
        "retry_after_ms": 1,
        "operation": {"stage": "publication"},
    }
    malformed_preparing = []
    for label, field, value in (
        ("invalid state", "state", "ready"),
        ("invalid operation", "operation", None),
        ("zero delay", "retry_after_ms", 0),
        ("boolean delay", "retry_after_ms", True),
    ):
        state = deepcopy(preparing)
        state[field] = value
        malformed_preparing.append((label, state))
    for label, state in malformed_preparing:
        hostile = ScriptedMcpProcess(
            [
                {
                    "result": {
                        "structuredContent": state,
                        "isError": False,
                    }
                }
            ]
        )
        try:
            hostile.tool_until_ready("ground", {}, f"self-test-preparing-{label}")
        except ProofFailure as exc:
            require(
                "malformed error envelope" in str(exc)
                or "invalid retry_after_ms" in str(exc),
                f"{label} preparing failure omitted its diagnostic: {exc}",
            )
        else:
            raise ProofFailure(f"preparing result with {label} was accepted")
        require(len(hostile.calls) == 1, f"preparing result with {label} was retried")


def run_installation_self_tests() -> None:
    _modern_protocol_initialization_test()
    query = "scripted-search"
    _readiness_convergence_test(query)
    _degraded_convergence_test(query)
    _terminal_unavailable_test(query)
    _hostile_result_tests(query)
