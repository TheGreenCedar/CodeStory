"""Owned subprocess execution, MCP transport, and temporary-directory cleanup."""

from __future__ import annotations

import json
import queue
import subprocess
import tempfile
import threading
import time
from pathlib import Path

from .foundation import (
    ENGINE_DIAGNOSTICS_URI,
    STATUS_URI,
    ProofFailure,
    project_resource_uri,
    require,
    resource_uri_matches,
)


def mcp_search_arguments(project: Path, query: str) -> dict[str, str]:
    """Build the closed public MCP search argument shape."""

    return {"project": str(project), "query": query}


def run(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> dict:
    started = time.perf_counter()
    # A packaged worker can start the resident embedding server and then exit.
    # Pipe capture makes communicate() wait for EOF from that descendant too,
    # even though the direct worker has finished. Regular files retain the same
    # output while letting subprocess.run() wait only for the process it owns.
    with (
        tempfile.TemporaryFile(mode="w+", encoding=None) as stdout_capture,
        tempfile.TemporaryFile(mode="w+", encoding=None) as stderr_capture,
    ):
        try:
            completed = subprocess.run(
                command,
                cwd=cwd,
                env=env,
                text=True,
                stdout=stdout_capture,
                stderr=stderr_capture,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as error:
            stdout_capture.flush()
            stderr_capture.flush()
            stdout_capture.buffer.seek(0)
            stderr_capture.buffer.seek(0)
            stdout = stdout_capture.buffer.read()
            stderr = stderr_capture.buffer.read()
            # TimeoutExpired retains raw bytes even when subprocess text mode
            # is enabled. Preserve that public shape as well as the output.
            error.timeout = timeout
            error.stdout = stdout or None
            error.stderr = stderr or None
            raise
        stdout_capture.seek(0)
        stderr_capture.seek(0)
        stdout = stdout_capture.read()
        stderr = stderr_capture.read()
    result = {
        "command": command,
        "exit_code": completed.returncode,
        "wall_ms": round((time.perf_counter() - started) * 1000, 3),
        "stdout": stdout,
        "stderr": stderr,
    }
    if completed.returncode != 0:
        stdout_tail = stdout[-2000:].strip()
        stderr_tail = stderr[-2000:].strip()
        details = "\n".join(
            part
            for part in (
                f"stdout:\n{stdout_tail}" if stdout_tail else "",
                f"stderr:\n{stderr_tail}" if stderr_tail else "",
            )
            if part
        )
        suffix = f"\n{details}" if details else ""
        raise ProofFailure(
            f"command failed ({completed.returncode}): {' '.join(command)}{suffix}"
        )
    return result


def json_command(
    command: list[str],
    *,
    env: dict[str, str],
    cwd: Path,
    timeout: int,
) -> tuple[dict, dict]:
    result = run(command, env=env, cwd=cwd, timeout=timeout)
    try:
        payload = json.loads(result["stdout"])
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"command did not emit JSON: {' '.join(command)}: {exc}"
        ) from exc
    require(
        isinstance(payload, dict),
        f"command emitted non-object JSON: {' '.join(command)}",
    )
    return result, payload


def extract_resource(
    response: dict,
    uri: str,
    *,
    platform_name: str | None = None,
    samefile=None,
) -> dict:
    require("error" not in response, f"resource read failed: {response.get('error')}")
    contents = response.get("result", {}).get("contents", [])
    for item in contents:
        if (
            isinstance(item, dict)
            and isinstance(item.get("uri"), str)
            and resource_uri_matches(
                uri,
                item["uri"],
                platform_name=platform_name,
                samefile=samefile,
            )
        ):
            payload = json.loads(item.get("text", "{}"))
            require(isinstance(payload, dict), "resource emitted non-object JSON")
            return payload
    raise ProofFailure(f"resource response did not contain {uri}")


_READINESS_CODE_STATES = frozenset(
    (
        ("codestory_preparing", "preparing"),
        ("codestory_updating", "updating"),
    )
)
_READINESS_KIND_STATES = frozenset(
    (
        ("preparing", "preparing"),
        ("updating", "updating"),
    )
)
_LEGACY_SEARCH_RETRIEVAL_STATES = frozenset(("ready", "degraded"))
_SCHEMA3_SEARCH_RETRIEVAL_STATES = frozenset(("full", "degraded"))
_SEARCH_CONVERGED_RETRIEVAL_STATES = frozenset(("ready", "full"))


def is_schema3_search_projection(state: dict) -> bool:
    """True when the payload is the agent evidence projection (schema_version 3)."""

    return state.get("schema_version") == 3


def resolve_search_snippet_anchor(state: dict) -> dict:
    """Resolve a snippet-capable node from legacy hits or schema-3 evidence.

    Returns ``{"node_id": str, "snippet_link_uri": str | None}``.

    Legacy installed-host proofs decorate resolvable hits with ``links`` that
    include a ``rel=snippet`` continuation URI. Live schema-3 search returns
    evidence rows with ``symbol_id`` and omits those links; callers then
    synthesize the project-bound snippet URI from ``node_id``. Fail closed on
    missing or malformed shapes rather than inventing an anchor.
    """

    require(
        isinstance(state, dict),
        f"MCP search returned a non-object projection for snippet anchoring: {state!r}",
    )
    if is_schema3_search_projection(state):
        require(
            state.get("kind") == "complete",
            f"MCP search did not return a complete schema-3 evidence projection: {state!r}",
        )
        evidence = state.get("evidence")
        require(
            isinstance(evidence, list),
            f"MCP search returned non-array evidence: {state!r}",
        )
        for row in evidence:
            if not isinstance(row, dict):
                continue
            symbol_id = row.get("symbol_id")
            if isinstance(symbol_id, str) and symbol_id:
                return {"node_id": symbol_id, "snippet_link_uri": None}
        raise ProofFailure(
            "packaged search omitted resolvable schema-3 evidence with "
            f"symbol_id: {state!r}"
        )

    hits = state.get("hits")
    require(
        isinstance(hits, list),
        f"MCP search returned non-array hits: {state!r}",
    )
    for hit in hits:
        if not isinstance(hit, dict):
            continue
        node_id = hit.get("node_id")
        links = hit.get("links")
        if not (
            isinstance(node_id, str)
            and node_id
            and isinstance(links, list)
        ):
            continue
        snippet_uri = next(
            (
                link.get("uri")
                for link in links
                if isinstance(link, dict)
                and link.get("rel") == "snippet"
                and isinstance(link.get("uri"), str)
                and link.get("uri")
            ),
            None,
        )
        if isinstance(snippet_uri, str):
            return {"node_id": node_id, "snippet_link_uri": snippet_uri}
    raise ProofFailure(
        "packaged search omitted a resolvable hit with continuation links: "
        f"{state!r}"
    )


def resolve_quality_search_hits(payload: dict) -> list:
    """Return ordered hits for publication quality ranking.

    Legacy CLI ``SearchOutput`` exposes ``indexed_symbol_hits``. Live
    agent-profile search emits schema-3 ``evidence`` rows and omits that
    field. Accept either shape; fail closed on incomplete or non-array
    projections rather than inventing hits.
    """

    require(
        isinstance(payload, dict),
        f"qualification search returned a non-object projection: {payload!r}",
    )
    if is_schema3_search_projection(payload):
        require(
            payload.get("kind") == "complete",
            "qualification search did not return a complete schema-3 "
            f"evidence projection: {payload!r}",
        )
        evidence = payload.get("evidence")
        require(
            isinstance(evidence, list),
            f"qualification search returned non-array evidence: {payload!r}",
        )
        return evidence

    hits = payload.get("indexed_symbol_hits")
    require(
        isinstance(hits, list),
        "qualification search omitted indexed symbol hits",
    )
    return hits


def quality_search_hit_matches(hit: object, expected: str) -> bool:
    """True when a legacy or schema-3 hit carries ``expected`` for ranking.

    Legacy rows match ``display_name``. Schema-3 evidence rows drop that
    field and keep ``excerpt`` / ``path``; qualification anchors appear in
    the pinned source window excerpt for indexed symbol hits.
    """

    if not isinstance(hit, dict) or not isinstance(expected, str) or not expected:
        return False
    for key in ("display_name", "excerpt", "path"):
        value = hit.get(key)
        if isinstance(value, str) and expected in value:
            return True
    return False


def search_retrieval_state(state: dict, *, query: object) -> str:
    """Validate an MCP search projection and return its retrieval.state.

    Legacy installed-host proofs echoed ``query`` / ``hits`` with
    ``retrieval.state∈{ready,degraded}``. Live schema-3 search returns an
    evidence envelope (``kind`` / ``evidence`` / ``retrieval.state∈{full,degraded}``)
    and omits those legacy fields. Accept either shape; never treat
    ``unavailable`` (or other non-ready states) as convergence.
    """

    if is_schema3_search_projection(state):
        require(
            state.get("kind") == "complete",
            f"MCP search did not return a complete schema-3 evidence projection: {state!r}",
        )
        require(
            isinstance(state.get("evidence"), list),
            f"MCP search returned non-array evidence: {state!r}",
        )
        retrieval = state.get("retrieval")
        require(
            isinstance(retrieval, dict)
            and retrieval.get("state") in _SCHEMA3_SEARCH_RETRIEVAL_STATES,
            f"MCP search did not return the ready installed retrieval projection: {state!r}",
        )
        return retrieval["state"]

    require(
        isinstance(query, str) and state.get("query") == query,
        f"MCP search returned a mismatched query: expected {query!r}, response={state!r}",
    )
    require(
        isinstance(state.get("hits"), list),
        f"MCP search returned non-array hits: {state!r}",
    )
    retrieval = state.get("retrieval")
    require(
        isinstance(retrieval, dict)
        and retrieval.get("state") in _LEGACY_SEARCH_RETRIEVAL_STATES,
        f"MCP search did not return the ready installed retrieval projection: {state!r}",
    )
    return retrieval["state"]


def tool_result_envelope(result: dict, *, name: str, attempt: int) -> dict:
    """Resolve the tool payload for MCP 2024-11-05 text envelopes and structuredContent.

    Protocol revisions that omit structuredContent on fail-open preparing still carry the
    same JSON object in content[0].text. Callers must treat those as equivalent; parsing
    here is not a readiness bypass for terminal failures.
    """

    structured = result.get("structuredContent")
    if isinstance(structured, dict):
        return structured
    content = result.get("content")
    if isinstance(content, list) and content:
        first = content[0]
        if isinstance(first, dict) and first.get("type") == "text":
            text = first.get("text")
            if isinstance(text, str) and text:
                try:
                    parsed = json.loads(text)
                except json.JSONDecodeError as exc:
                    raise ProofFailure(
                        f"MCP {name} attempt {attempt} returned non-JSON text content: {result!r}"
                    ) from exc
                if isinstance(parsed, dict):
                    return parsed
    raise ProofFailure(
        f"MCP {name} attempt {attempt} returned non-object structuredContent: {result!r}"
    )


def is_readiness_retry_envelope(envelope: dict) -> bool:
    return (envelope.get("code"), envelope.get("state")) in _READINESS_CODE_STATES or (
        envelope.get("kind"),
        envelope.get("state"),
    ) in _READINESS_KIND_STATES


def readiness_retry_after_ms(envelope: dict) -> int | None:
    retry_after_ms = envelope.get("retry_after_ms")
    if (
        isinstance(retry_after_ms, int)
        and not isinstance(retry_after_ms, bool)
        and retry_after_ms >= 0
    ):
        return retry_after_ms
    minimum_next = envelope.get("minimum_next")
    if isinstance(minimum_next, dict):
        after_ms = minimum_next.get("after_ms")
        if (
            isinstance(after_ms, int)
            and not isinstance(after_ms, bool)
            and after_ms >= 0
        ):
            return after_ms
    return None


def allows_same_request_retry(envelope: dict, tool_name: str) -> bool:
    if envelope.get("retry_tool") == tool_name:
        return True
    minimum_next = envelope.get("minimum_next")
    return (
        isinstance(minimum_next, dict)
        and minimum_next.get("kind") == "retry_same_request"
    )


def attach_structured_content(response: dict, envelope: dict) -> dict:
    """Expose a parsed text envelope as structuredContent for proof callers."""

    result = response.get("result")
    if not isinstance(result, dict):
        return response
    if isinstance(result.get("structuredContent"), dict):
        return response
    normalized = dict(response)
    normalized_result = dict(result)
    normalized_result["structuredContent"] = envelope
    normalized["result"] = normalized_result
    return normalized


class McpProcess:
    def __init__(
        self,
        command: list[str],
        *,
        env: dict[str, str],
        cwd: Path,
        timeout: int,
    ):
        self.timeout = timeout
        self.process = subprocess.Popen(
            command,
            cwd=cwd,
            env=env,
            text=True,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.stderr: list[str] = []
        assert self.process.stdout and self.process.stderr and self.process.stdin
        threading.Thread(
            target=self._reader,
            args=(self.process.stdout, self.lines),
            daemon=True,
        ).start()
        threading.Thread(target=self._stderr_reader, daemon=True).start()
        self.transcript: list[dict] = []
        self.tool_attempt_counts: dict[str, int] = {}

    @staticmethod
    def _reader(stream, output: queue.Queue[str | None]) -> None:
        for line in stream:
            output.put(line)
        output.put(None)

    def _stderr_reader(self) -> None:
        assert self.process.stderr
        self.stderr.extend(self.process.stderr.readlines())

    def send(self, request: dict, deadline: float | None = None) -> dict:
        assert self.process.stdin
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        # A caller that already owns a bound threads it in; otherwise this call owns its own.
        # Minting a fresh full budget underneath a caller's deadline is how a readiness loop
        # burns several times the declared timeout: the loop only re-checks its bound between
        # transport waits, so one late request can add another whole timeout past it.
        if deadline is None:
            deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"MCP request timed out: {request.get('id')}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty as exc:
                raise ProofFailure(
                    f"MCP request timed out: {request.get('id')}"
                ) from exc
            require(
                line is not None,
                f"MCP process closed: {''.join(self.stderr)[-2000:]}",
            )
            response = json.loads(line)
            self.transcript.append({"request": request, "response": response})
            if response.get("id") == request.get("id"):
                return response

    def initialize(self) -> None:
        response = self.send(
            {
                "jsonrpc": "2.0",
                "id": "initialize",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "packaged-proof",
                        "version": "1",
                    },
                },
            }
        )
        require(
            "error" not in response, f"MCP initialize failed: {response.get('error')}"
        )
        assert self.process.stdin
        self.process.stdin.write(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                }
            )
            + "\n"
        )
        self.process.stdin.flush()

    def status(self, project: Path, request_id: str) -> dict:
        uri = project_resource_uri(STATUS_URI, project)
        return extract_resource(
            self.send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "resources/read",
                    "params": {"uri": uri},
                }
            ),
            uri,
        )

    def engine_diagnostics(self, project: Path, request_id: str) -> dict:
        uri = project_resource_uri(ENGINE_DIAGNOSTICS_URI, project)
        return extract_resource(
            self.send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "resources/read",
                    "params": {"uri": uri},
                }
            ),
            uri,
        )

    def resource(self, uri: str, request_id: str) -> dict:
        return extract_resource(
            self.send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": "resources/read",
                    "params": {"uri": uri},
                }
            ),
            uri,
        )

    def tool(
        self,
        name: str,
        arguments: dict,
        request_id: str,
        deadline: float | None = None,
    ) -> dict:
        response = self.send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            },
            deadline=deadline,
        )
        require("error" not in response, f"MCP {name} failed: {response.get('error')}")
        return response

    def tool_until_ready(
        self,
        name: str,
        arguments: dict,
        request_id: str,
        deadline: float | None = None,
    ) -> tuple[dict, int]:
        # A caller that already owns a bound threads it in; otherwise this call owns its own.
        # The same bound has to reach the transport, or each retry's request mints a fresh
        # budget and the readiness loop overruns whatever deadline it was handed.
        if deadline is None:
            deadline = time.monotonic() + self.timeout
        attempt = 0
        while True:
            attempt += 1
            self.tool_attempt_counts[request_id] = attempt
            response = self.tool(
                name, arguments, f"{request_id}-{attempt}", deadline=deadline
            )
            result = response.get("result")
            require(
                isinstance(result, dict),
                f"MCP {name} attempt {attempt} returned a non-object result: {result!r}",
            )
            state = tool_result_envelope(result, name=name, attempt=attempt)
            if is_readiness_retry_envelope(state):
                self._wait_for_readiness_retry(
                    name,
                    attempt,
                    state,
                    deadline,
                )
                continue
            if result.get("isError") is True:
                require(
                    False,
                    f"MCP {name} attempt {attempt} returned a terminal or malformed error envelope: {state!r}",
                )
            return attach_structured_content(response, state), attempt

    def _wait_for_readiness_retry(
        self,
        name: str,
        attempt: int,
        state: dict,
        deadline: float,
    ) -> None:
        require(
            is_readiness_retry_envelope(state),
            f"MCP {name} attempt {attempt} returned a terminal or malformed error envelope: {state!r}",
        )
        require(
            allows_same_request_retry(state, name),
            f"MCP {name} attempt {attempt} returned the wrong retry tool: {state!r}",
        )
        retry_after_ms = readiness_retry_after_ms(state)
        require(
            retry_after_ms is not None,
            f"MCP {name} attempt {attempt} returned invalid retry_after_ms: {state!r}",
        )
        remaining = deadline - time.monotonic()
        require(
            remaining > 0,
            f"MCP {name} did not become ready after attempt {attempt}: {state!r}",
        )
        time.sleep(min(retry_after_ms, max(0, int(remaining * 1000))) / 1000)

    def search_until_ready(self, arguments: dict, request_id: str) -> tuple[dict, int]:
        deadline = time.monotonic() + self.timeout
        total_attempts = 0
        poll = 0
        while True:
            poll += 1
            poll_request_id = (
                request_id if poll == 1 else f"{request_id}-degraded-{poll}"
            )
            response, attempts = self.tool_until_ready(
                "search", arguments, poll_request_id, deadline=deadline
            )
            total_attempts += attempts
            self.tool_attempt_counts[request_id] = total_attempts
            state = response["result"]["structuredContent"]
            retrieval_state = search_retrieval_state(
                state, query=arguments.get("query")
            )
            if retrieval_state in _SEARCH_CONVERGED_RETRIEVAL_STATES:
                return response, total_attempts
            # The projection reports the real retrieval state, so a fresh install
            # answers lexically while the semantic sidecar is still publishing.
            # That degraded window is convergence, not failure: keep asking until
            # the shared deadline, and let a host that never converges fail loud.
            # Schema-3 uses retrieval.state=full once hybrid is published; degraded
            # still means "keep polling", never "pass".
            remaining = deadline - time.monotonic()
            require(
                remaining > 0,
                f"MCP search retrieval projection never became ready: {state!r}",
            )
            time.sleep(min(1.0, remaining))

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)


def add_exception_note(error: BaseException, note: str) -> None:
    add_note = getattr(error, "add_note", None)
    if callable(add_note):
        add_note(note)
        return
    notes = list(getattr(error, "__notes__", []))
    notes.append(note)
    error.__notes__ = notes
    if error.args:
        error.args = (f"{error.args[0]}\nsecondary context: {note}", *error.args[1:])
    else:
        error.args = (f"secondary context: {note}",)


class FailurePreservingTemporaryDirectory(tempfile.TemporaryDirectory):
    def __init__(
        self,
        *args,
        cleanup_retry_budget_secs: float = 0,
        cleanup_retry_interval_secs: float = 0.5,
        **kwargs,
    ):
        super().__init__(*args, **kwargs)
        self.cleanup_retry_budget_secs = cleanup_retry_budget_secs
        self.cleanup_retry_interval_secs = cleanup_retry_interval_secs

    def __exit__(self, exc_type, exc, traceback) -> bool | None:
        deadline = time.monotonic() + self.cleanup_retry_budget_secs
        try:
            while True:
                try:
                    self.cleanup()
                    return None
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(
                        min(
                            self.cleanup_retry_interval_secs,
                            max(0, deadline - time.monotonic()),
                        )
                    )
        except OSError as cleanup_error:
            if exc is None:
                raise
            add_exception_note(
                exc,
                f"temporary package directory cleanup also failed: {cleanup_error}",
            )
            return False
