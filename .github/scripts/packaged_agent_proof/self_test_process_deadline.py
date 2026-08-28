"""Readiness-wait deadline self-tests for the owned MCP transport."""

from __future__ import annotations

import hashlib
import json
import queue
from types import SimpleNamespace
from unittest.mock import patch

from . import subprocess_control
from .foundation import ProofFailure, require
from .subprocess_control import McpProcess

_RETRY_AFTER_MS = 30_000
_TIMEOUT_SECS = 60.0
_NEVER = float("inf")


class _VirtualClock:
    """Deterministic stand-in for the module clock so the legs cost no wall time."""

    def __init__(self) -> None:
        self.now = 0.0

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += max(0.0, float(seconds))


class _ScriptedHost(McpProcess):
    """An McpProcess whose stdio is scripted instead of a real subprocess.

    The script replaces the pipes, not the methods, so ``tool`` and ``send`` are the shipped
    implementations. A leg that only stubbed ``tool`` could not see the transport's own
    deadline, which is the other place a readiness wait can mint a fresh budget.
    """

    def __init__(
        self,
        clock: _VirtualClock,
        timeout: float,
        script: list[str],
        latencies: list[float] | None = None,
    ) -> None:
        self.clock = clock
        self.timeout = timeout
        # The last scripted step and latency repeat, so a leg only names its distinct steps.
        self.script = script
        self.latencies = latencies or [0.0]
        self.reads = 0
        self.pending: dict | None = None
        self.stderr: list[str] = []
        self.transcript: list[dict] = []
        self.tool_attempt_counts: dict[str, int] = {}
        self.lines = self
        self.process = SimpleNamespace(stdin=self)

    # --- stdin stand-in -------------------------------------------------------------------
    def write(self, payload: str) -> None:
        self.pending = json.loads(payload)

    def flush(self) -> None:
        return None

    # --- stdout queue stand-in ------------------------------------------------------------
    def get(self, timeout: float | None = None):
        self.reads += 1
        step = self.script[min(self.reads, len(self.script)) - 1]
        latency = self.latencies[min(self.reads, len(self.latencies)) - 1]
        if timeout is not None and latency > timeout:
            # The transport waited its whole remaining bound and the host never answered.
            self.clock.now += max(0.0, timeout)
            raise queue.Empty
        self.clock.now += latency
        return json.dumps(self._response(step))

    def _response(self, step: str) -> dict:
        assert self.pending is not None
        params = self.pending.get("params", {})
        if step == "preparing":
            return {
                "jsonrpc": "2.0",
                "id": self.pending.get("id"),
                "result": {
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": _RETRY_AFTER_MS,
                        "operation": {"stage": "publication"},
                    },
                    "isError": False,
                },
            }
        query = params.get("arguments", {}).get("query", "")
        retrieval_state = "full" if step == "ready" else step
        retrieval_generation = (
            "retrieval-deadline-self-test" if retrieval_state == "full" else None
        )
        return {
            "jsonrpc": "2.0",
            "id": self.pending.get("id"),
            "result": {
                "structuredContent": {
                    "kind": "complete",
                    "schema_version": 3,
                    "identity": {
                        "packet_id": "packet-deadline-self-test",
                        "request_id": "request-deadline-self-test",
                        "question_sha256": hashlib.sha256(
                            query.encode("utf-8")
                        ).hexdigest(),
                    },
                    "publication": {
                        "core": {
                            "project_id": "project-deadline-self-test",
                            "generation_id": "core-generation-deadline-self-test",
                            "run_id": "core-run-deadline-self-test",
                        },
                        "retrieval": (
                            {
                                "core_generation_id": "core-generation-deadline-self-test",
                                "core_run_id": "core-run-deadline-self-test",
                                "retrieval_generation": retrieval_generation,
                                "retrieval_input_sha256": "a" * 64,
                                "semantic_generation": "semantic-deadline-self-test",
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
                },
                "isError": False,
            },
        }


def _run_shared_deadline_leg() -> None:
    clock = _VirtualClock()
    # A degraded poll lands mid-window, so the next poll's readiness retries are the only
    # thing that can push the wait past the shared bound.
    host = _ScriptedHost(clock, _TIMEOUT_SECS, ["preparing", "degraded", "preparing"])
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


def _run_transport_deadline_leg() -> None:
    clock = _VirtualClock()
    # The first answer arrives late enough that a request minting its own full budget would
    # outlive the shared bound. The host then goes silent, so the transport wait is the only
    # thing left that can overrun.
    host = _ScriptedHost(
        clock,
        _TIMEOUT_SECS,
        ["preparing", "silent"],
        [10.0, _NEVER],
    )
    with patch.object(subprocess_control, "time", clock):
        try:
            host.search_until_ready({"query": "self-test"}, "search")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("search_until_ready did not fail on a host that stopped answering")
    require(
        clock.now <= _TIMEOUT_SECS,
        f"a request under search_until_ready minted its own budget: waited {clock.now}s "
        f"against its {_TIMEOUT_SECS}s bound",
    )


def _run_default_deadline_leg() -> None:
    clock = _VirtualClock()
    # A host that never becomes ready pins the default bound in both directions: a default
    # that grew past self.timeout keeps retrying past _TIMEOUT_SECS, and one that shrank gives
    # up before it.
    host = _ScriptedHost(clock, _TIMEOUT_SECS, ["preparing"])
    with patch.object(subprocess_control, "time", clock):
        try:
            host.tool_until_ready("search", {"query": "self-test"}, "search")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("tool_until_ready did not fail on a host that never became ready")
    attempts = host.tool_attempt_counts.get("search")
    require(
        attempts == 3 and clock.now == _TIMEOUT_SECS,
        f"tool_until_ready without a deadline changed its own bound: {attempts} attempts "
        f"over {clock.now}s against {_TIMEOUT_SECS}s",
    )


def _run_converging_host_leg() -> None:
    clock = _VirtualClock()
    host = _ScriptedHost(clock, _TIMEOUT_SECS, ["preparing", "ready"])
    with patch.object(subprocess_control, "time", clock):
        _, attempts = host.tool_until_ready("search", {"query": "self-test"}, "search")
    require(
        attempts == 2 and clock.now <= _TIMEOUT_SECS,
        f"tool_until_ready gave up on a host that converged inside the bound: {attempts} "
        f"attempts over {clock.now}s",
    )


def _run_threaded_deadline_leg() -> None:
    clock = _VirtualClock()
    host = _ScriptedHost(clock, _TIMEOUT_SECS, ["preparing"])
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


def _run_threaded_transport_deadline_leg() -> None:
    clock = _VirtualClock()
    # A caller-owned deadline has to reach the transport too, not only the readiness retries.
    host = _ScriptedHost(clock, _TIMEOUT_SECS, ["silent"], [_NEVER])
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
        f"the transport ignored a caller-owned 5.0s deadline and waited {clock.now}s",
    )


def run_process_deadline_self_tests() -> None:
    _run_shared_deadline_leg()
    _run_transport_deadline_leg()
    _run_default_deadline_leg()
    _run_converging_host_leg()
    _run_threaded_deadline_leg()
    _run_threaded_transport_deadline_leg()
