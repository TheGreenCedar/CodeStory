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
            state = result.get("structuredContent")
            require(
                isinstance(state, dict),
                f"MCP {name} attempt {attempt} returned non-object structuredContent: {result!r}",
            )
            is_error = result.get("isError")
            if "isError" not in result or is_error is False:
                return response, attempt
            self._wait_for_readiness_retry(
                name,
                attempt,
                state,
                is_error,
                deadline,
            )

    def _wait_for_readiness_retry(
        self,
        name: str,
        attempt: int,
        state: dict,
        is_error: object,
        deadline: float,
    ) -> None:
        require(
            is_error is True,
            f"MCP {name} attempt {attempt} returned invalid isError={is_error!r}: {state!r}",
        )
        require(
            (state.get("code"), state.get("state"))
            in (
                ("codestory_preparing", "preparing"),
                ("codestory_updating", "updating"),
            ),
            f"MCP {name} attempt {attempt} returned a terminal or malformed error envelope: {state!r}",
        )
        require(
            state.get("retry_tool") == name,
            f"MCP {name} attempt {attempt} returned the wrong retry tool: {state!r}",
        )
        retry_after_ms = state.get("retry_after_ms")
        require(
            isinstance(retry_after_ms, int)
            and not isinstance(retry_after_ms, bool)
            and retry_after_ms >= 0,
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
            query = arguments.get("query")
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
                and retrieval.get("state") in ("ready", "degraded"),
                f"MCP search did not return the ready installed retrieval projection: {state!r}",
            )
            if retrieval.get("state") == "ready":
                return response, total_attempts
            # The projection reports the real retrieval state, so a fresh install
            # answers lexically while the semantic sidecar is still publishing.
            # That degraded window is convergence, not failure: keep asking until
            # the shared deadline, and let a host that never converges fail loud.
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
