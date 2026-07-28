"""Readiness-wait deadline self-tests for the owned MCP transport."""

from __future__ import annotations

from unittest.mock import patch

from . import subprocess_control
from .foundation import ProofFailure, require
from .subprocess_control import McpProcess

_RETRY_AFTER_MS = 30_000
_TIMEOUT_SECS = 60.0


class _VirtualClock:
    """Deterministic stand-in for the module clock so the legs cost no wall time."""

    def __init__(self) -> None:
        self.now = 0.0

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += max(0.0, float(seconds))


class _ScriptedHost(McpProcess):
    """An McpProcess whose tool calls replay a script instead of a real subprocess."""

    def __init__(self, timeout: float, script: list[str]) -> None:
        self.timeout = timeout
        self.script = script
        self.calls = 0
        self.transcript: list[dict] = []
        self.tool_attempt_counts: dict[str, int] = {}

    def tool(self, name: str, arguments: dict, request_id: str) -> dict:
        self.calls += 1
        # The last scripted step repeats so a leg only has to name its distinct steps.
        step = self.script[min(self.calls, len(self.script)) - 1]
        if step == "preparing":
            return {
                "result": {
                    "isError": True,
                    "structuredContent": {
                        "code": "codestory_preparing",
                        "state": "preparing",
                        "retry_tool": name,
                        "retry_after_ms": _RETRY_AFTER_MS,
                    },
                }
            }
        return {
            "result": {
                "structuredContent": {
                    "query": arguments.get("query"),
                    "hits": [],
                    "retrieval": {"state": step},
                }
            }
        }


def _run_shared_deadline_leg() -> None:
    clock = _VirtualClock()
    # A degraded poll lands mid-window, so the next poll's readiness retries are the only
    # thing that can push the wait past the shared bound.
    host = _ScriptedHost(_TIMEOUT_SECS, ["preparing", "degraded", "preparing"])
    with patch.object(subprocess_control, "time", clock):
        try:
            host.search_until_ready({"query": "self-test"}, "search")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("search_until_ready did not fail on a host that never converged")
    require(
        clock.now <= _TIMEOUT_SECS,
        f"search_until_ready waited {clock.now}s against its {_TIMEOUT_SECS}s bound",
    )


def _run_default_deadline_leg() -> None:
    clock = _VirtualClock()
    host = _ScriptedHost(_TIMEOUT_SECS, ["preparing", "preparing", "ready"])
    with patch.object(subprocess_control, "time", clock):
        _, attempts = host.tool_until_ready("search", {"query": "self-test"}, "search")
    require(
        attempts == 3 and clock.now == _TIMEOUT_SECS,
        f"tool_until_ready without a deadline changed its own bound: {attempts} attempts "
        f"over {clock.now}s",
    )


def _run_threaded_deadline_leg() -> None:
    clock = _VirtualClock()
    host = _ScriptedHost(_TIMEOUT_SECS, ["preparing"])
    with patch.object(subprocess_control, "time", clock):
        try:
            host.tool_until_ready(
                "search",
                {"query": "self-test"},
                "search",
                deadline=clock.monotonic() + 5.0,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("tool_until_ready ignored a caller-owned deadline")
    require(
        clock.now <= 5.0,
        f"tool_until_ready waited {clock.now}s against a caller-owned 5.0s deadline",
    )


def run_process_deadline_self_tests() -> None:
    _run_shared_deadline_leg()
    _run_default_deadline_leg()
    _run_threaded_deadline_leg()
