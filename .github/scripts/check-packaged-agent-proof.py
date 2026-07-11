#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import queue
import signal
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import textwrap
import time
import zipfile
from pathlib import Path


DEFAULT_QUESTION = "Explain how CodeStory validates packaged agent readiness."
DEFAULT_QUERY = "RuntimeContext"
STATUS_URI = "codestory://status"
AGENT_GUIDE_URI = "codestory://agent-guide"
SERVER_RESOURCE_URIS = (STATUS_URI, AGENT_GUIDE_URI)
PLUGIN_SKILL_RELATIVE = Path("plugins/codestory/skills/codestory-grounding/SKILL.md")


class GateFailure(Exception):
    def __init__(self, layer: str, artifact: Path, message: str):
        super().__init__(message)
        self.layer = layer
        self.artifact = artifact
        self.message = message


def fail(layer: str, artifact: Path, message: str) -> None:
    raise GateFailure(layer, artifact, message)


def ensure_inside(path: Path, root: Path) -> None:
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f"archive member escapes extraction root: {path}")


def unpack_archive(archive: Path, destination: Path) -> None:
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                target = destination / member.filename
                ensure_inside(target, destination)
            handle.extractall(destination)
        return

    if tarfile.is_tarfile(archive):
        with tarfile.open(archive) as handle:
            for member in handle.getmembers():
                target = destination / member.name
                ensure_inside(target, destination)
            handle.extractall(destination)
        return

    raise ValueError(f"unsupported archive format: {archive}")


def find_cli(unpacked: Path) -> Path:
    names = {"codestory-cli", "codestory-cli.exe", "codestory-cli.cmd"}
    matches = [path for path in unpacked.rglob("*") if path.is_file() and path.name in names]
    if not matches:
        raise FileNotFoundError(f"archive does not contain codestory-cli under {unpacked}")
    cli = sorted(matches, key=lambda path: (len(path.parts), str(path)))[0]
    if cli.suffix != ".cmd":
        cli.chmod(cli.stat().st_mode | stat.S_IXUSR)
    return cli


def find_plugin_skill(unpacked: Path) -> Path:
    matches = [
        path
        for path in unpacked.rglob("SKILL.md")
        if Path(*path.relative_to(unpacked).parts[-len(PLUGIN_SKILL_RELATIVE.parts) :])
        == PLUGIN_SKILL_RELATIVE
    ]
    if not matches:
        raise FileNotFoundError(f"archive does not contain {PLUGIN_SKILL_RELATIVE.as_posix()}")
    return sorted(matches, key=lambda path: (len(path.parts), str(path)))[0]


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def read_json_file(path: Path) -> object:
    return json.loads(path.read_text(encoding="utf-8"))


def captured_text(value: str | bytes | None) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value or ""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_archive_checksum(archive: Path, checksum_file: Path, artifact: Path) -> None:
    lines = checksum_file.read_text(encoding="utf-8").splitlines()
    expected = next(
        (
            line.split(maxsplit=1)[0].lower()
            for line in lines
            if len(line.split(maxsplit=1)) == 2
            and line.split(maxsplit=1)[1].lstrip("*").strip() == archive.name
        ),
        None,
    )
    actual = sha256_file(archive)
    write_json(
        artifact,
        {"archive": str(archive), "checksum_file": str(checksum_file), "expected": expected, "actual": actual},
    )
    require(expected is not None, "checksum", artifact, f"checksum file does not list {archive.name}")
    require(actual == expected, "checksum", artifact, f"checksum mismatch for {archive.name}")


def cleanup_proof_cache(cli: Path, project: Path, cache_root: Path) -> None:
    env = {**os.environ, "CODESTORY_CACHE_ROOT": str(cache_root)}
    for profile, extra in (("local", []), ("agent", ["--run-id", "shared-agent"])):
        try:
            subprocess.run(
                [
                    str(cli),
                    "retrieval",
                    "down",
                    "--project",
                    str(project),
                    "--profile",
                    profile,
                    *extra,
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=20,
                check=False,
                env=env,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
    shutil.rmtree(cache_root, ignore_errors=True)


def require_plugin_manifest_version(plugin_root: Path, expected_version: str) -> None:
    manifest_path = plugin_root / ".codex-plugin" / "plugin.json"
    require(manifest_path.is_file(), "plugin_manifest", manifest_path, f"plugin manifest is missing: {manifest_path}")
    manifest = read_json_file(manifest_path)
    require(isinstance(manifest, dict), "plugin_manifest", manifest_path, "plugin manifest is not a JSON object")
    actual = manifest.get("version")
    require(
        actual == expected_version,
        "plugin_manifest",
        manifest_path,
        f"plugin manifest version is {actual!r}, expected {expected_version!r}",
    )


def run_command(
    cli: Path,
    layer: str,
    args: list[str],
    artifact: Path,
    timeout_secs: int,
    parse_json: bool = True,
    env: dict[str, str] | None = None,
) -> object | None:
    stdout_path = artifact.with_suffix(artifact.suffix + ".stdout.txt")
    stderr_path = artifact.with_suffix(artifact.suffix + ".stderr.txt")
    command = [str(cli), *args]
    try:
        result = subprocess.run(
            command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout_secs,
            check=False,
            env=env,
        )
    except subprocess.TimeoutExpired as exc:
        stdout_path.write_text(captured_text(exc.stdout), encoding="utf-8")
        stderr_path.write_text(captured_text(exc.stderr), encoding="utf-8")
        fail(layer, stdout_path, f"command timed out after {timeout_secs}s: {' '.join(command)}")

    stdout_path.write_text(result.stdout, encoding="utf-8")
    stderr_path.write_text(result.stderr, encoding="utf-8")
    if result.returncode != 0:
        artifact_path = artifact if artifact.is_file() and artifact.stat().st_size > 0 else (
            stderr_path if result.stderr.strip() else stdout_path
        )
        fail(layer, artifact_path, f"command exited {result.returncode}: {' '.join(command)}")

    if not parse_json:
        artifact.write_text(result.stdout, encoding="utf-8")
        return None

    if artifact.exists() and artifact.stat().st_size > 0:
        try:
            return read_json_file(artifact)
        except json.JSONDecodeError as exc:
            fail(layer, artifact, f"output file is not valid JSON: {exc}")

    try:
        parsed = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        fail(layer, stdout_path, f"stdout is not valid JSON: {exc}")
    write_json(artifact, parsed)
    return parsed


def require(value: bool, layer: str, artifact: Path, message: str) -> None:
    if not value:
        fail(layer, artifact, message)


def require_agent_ready(payload: object, layer: str, artifact: Path) -> None:
    verdicts = payload.get("verdicts", []) if isinstance(payload, dict) else []
    agent = next((item for item in verdicts if item.get("goal") == "agent_packet_search"), None)
    require(agent is not None, layer, artifact, "missing agent_packet_search readiness verdict")
    require(
        agent.get("status") == "ready",
        layer,
        artifact,
        f"agent_packet_search status is {agent.get('status')!r}, expected 'ready'",
    )
    require(isinstance(agent.get("summary"), str), layer, artifact, "agent readiness missing summary")
    require(isinstance(agent.get("minimum_next"), list), layer, artifact, "agent readiness missing minimum_next")
    require(isinstance(agent.get("full_repair"), list), layer, artifact, "agent readiness missing full_repair")


def require_retrieval_full(payload: object, layer: str, artifact: Path) -> None:
    mode = payload.get("retrieval_mode") if isinstance(payload, dict) else None
    if mode is None and isinstance(payload, dict):
        mode = payload.get("sidecar_retrieval", {}).get("retrieval_mode")
    require(mode == "full", layer, artifact, f"retrieval_mode is {mode!r}, expected 'full'")


def require_version(output: str, expected_version: str, archive: Path, artifact: Path) -> None:
    actual = output.strip().removeprefix("codestory-cli ").strip()
    require(
        actual == expected_version,
        "version",
        artifact,
        f"codestory-cli version is {actual!r}, expected {expected_version!r}",
    )
    expected_archive_prefix = f"codestory-cli-v{expected_version}-"
    require(
        archive.name.startswith(expected_archive_prefix),
        "version",
        artifact,
        f"archive name {archive.name!r} does not start with {expected_archive_prefix!r}",
    )


def require_help(output: str, artifact: Path) -> None:
    require("Usage:" in output, "help", artifact, "codestory-cli --help output missing Usage")
    require("codestory-cli" in output, "help", artifact, "codestory-cli --help output missing binary name")


def require_retrieval_index_ready(payload: object, layer: str, artifact: Path) -> None:
    require(isinstance(payload, dict), layer, artifact, "retrieval index output is not a JSON object")
    for field in ["zoekt_stubbed", "qdrant_stubbed", "scip_stubbed"]:
        value = payload.get(field)
        require(value is False, layer, artifact, f"{field} is {value!r}, expected False")


def require_search_full(payload: object, artifact: Path) -> None:
    shadow = payload.get("retrieval_shadow") if isinstance(payload, dict) else None
    mode = shadow.get("retrieval_mode") if isinstance(shadow, dict) else None
    require(mode == "full", "search", artifact, f"search retrieval_shadow.retrieval_mode is {mode!r}")


def require_packet_ready(payload: object, artifact: Path) -> None:
    require(isinstance(payload, dict), "packet", artifact, "packet output is not a JSON object")
    sufficiency = payload.get("sufficiency")
    require(isinstance(sufficiency, dict), "packet", artifact, "packet output missing sufficiency")
    status = sufficiency.get("status")
    require(status == "sufficient", "packet", artifact, f"packet sufficiency.status is {status!r}")
    require("retrieval_trace_summary" in payload, "packet", artifact, "packet output missing retrieval trace summary")
    answer = payload.get("answer")
    require(isinstance(answer, dict), "packet", artifact, "packet output missing answer")
    retrieval_version = answer.get("retrieval_version")
    require(
        retrieval_version == "sidecar",
        "packet",
        artifact,
        f"packet answer.retrieval_version is {retrieval_version!r}",
    )


def require_context_ready(payload: object, artifact: Path) -> None:
    require(isinstance(payload, dict), "context", artifact, "context output is not a JSON object")
    context = payload.get("context")
    require(isinstance(context, dict), "context", artifact, "context output missing context object")
    retrieval_version = context.get("retrieval_version")
    require(
        retrieval_version == "sidecar",
        "context",
        artifact,
        f"context retrieval_version is {retrieval_version!r}",
    )
    trace = context.get("retrieval_trace")
    require(isinstance(trace, dict), "context", artifact, "context output missing context retrieval trace")
    steps = trace.get("steps")
    require(
        isinstance(steps, list) and len(steps) > 0,
        "context",
        artifact,
        "context retrieval trace has no steps",
    )
    shadow = trace.get("retrieval_shadow")
    require(isinstance(shadow, dict), "context", artifact, "context retrieval trace missing retrieval shadow")
    mode = shadow.get("retrieval_mode")
    require(mode == "full", "context", artifact, f"context retrieval_shadow.retrieval_mode is {mode!r}")


def write_stdio_artifact(artifact: Path, transcript: list[dict], stdout: str, stderr_path: Path, extra: dict | None = None) -> None:
    payload = {
        "transcript": transcript,
        "stdout": stdout,
        "stderr_artifact": str(stderr_path),
    }
    if extra:
        payload.update(extra)
    write_json(artifact, payload)


def stream_lines(stream, lines: list[str], line_queue: queue.Queue[str | None] | None = None) -> None:
    try:
        for line in iter(stream.readline, ""):
            lines.append(line)
            if line_queue is not None:
                line_queue.put(line)
    finally:
        if line_queue is not None:
            line_queue.put(None)


def terminate_process_tree(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        try:
            subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=1,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    if process.poll() is None:
        process.kill()


def terminate_worker_pid(pid: int) -> None:
    if pid <= 0:
        return
    if os.name == "nt":
        try:
            subprocess.run(
                ["taskkill", "/PID", str(pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=2,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
    else:
        try:
            os.kill(pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass


def running_status_worker_pids(status: dict) -> set[int]:
    operations = status.get("readiness_broker", {}).get("operations", [])
    return {
        operation["pid"]
        for operation in operations
        if isinstance(operation, dict)
        and operation.get("status") == "running"
        and isinstance(operation.get("pid"), int)
    }


def terminate_status_workers(status: dict) -> None:
    for worker_pid in running_status_worker_pids(status):
        terminate_worker_pid(worker_pid)


def read_stdio_line(stdout_queue: queue.Queue[str | None], timeout_secs: int) -> str | None:
    deadline = time.monotonic() + timeout_secs
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise subprocess.TimeoutExpired("serve --stdio response", timeout_secs)
        try:
            line = stdout_queue.get(timeout=remaining)
        except queue.Empty as exc:
            raise subprocess.TimeoutExpired("serve --stdio response", timeout_secs) from exc
        if line is None:
            return None
        if line.strip():
            return line


def stdio_status(
    cli: Path,
    project: Path,
    artifact: Path,
    timeout_secs: int,
    env: dict[str, str] | None = None,
    cleanup_status_workers: bool = False,
) -> dict:
    return stdio_status_command(
        [str(cli), "serve", "--stdio", "--refresh", "none", "--project", str(project)],
        artifact,
        timeout_secs,
        project,
        env=env,
        cleanup_status_workers=cleanup_status_workers,
    )


def plugin_stdio_status(
    plugin_root: Path,
    release_dir: Path,
    project: Path,
    artifact: Path,
    timeout_secs: int,
    expected_version: str,
    cache_root: Path,
) -> dict:
    require_plugin_manifest_version(plugin_root, expected_version)
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), "plugin_stdio", artifact, f"plugin launcher is missing: {launcher}")
    with tempfile.TemporaryDirectory(prefix="codestory-plugin-data-", dir=artifact.parent) as data:
        return stdio_status_command(
            ["node", str(launcher)],
            artifact,
            timeout_secs,
            project,
            layer="plugin_stdio",
            cwd=project,
            env={
                **os.environ,
                "CODESTORY_CLI": "",
                "CODESTORY_CACHE_ROOT": str(cache_root),
                "CODESTORY_PLUGIN_RELEASE_DIR": str(release_dir),
                "PLUGIN_DATA": data,
            },
            cleanup_status_workers=True,
        )


def plugin_stdio_handoff(
    plugin_root: Path,
    release_dir: Path,
    project: Path,
    cache_root: Path,
    artifact: Path,
    timeout_secs: int,
    expected_version: str,
    archive_cli: Path,
) -> dict:
    require_plugin_manifest_version(plugin_root, expected_version)
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), "managed_plugin_handoff", artifact, f"plugin launcher is missing: {launcher}")
    with tempfile.TemporaryDirectory(prefix="codestory-plugin-data-", dir=artifact.parent) as data:
        plugin_data = Path(data)
        policy_path = plugin_data / "sidecar-setup-policy.json"
        policy_artifact = artifact.with_name("managed-plugin-policy.json")
        try:
            policy = subprocess.run(
                ["node", str(launcher), "sidecar-policy", "enable", "--policy-file", str(policy_path)],
                cwd=project,
                env={**os.environ, "PLUGIN_DATA": str(plugin_data)},
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout_secs,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            write_json(
                policy_artifact,
                {
                    "error": str(exc),
                    "stdout": captured_text(getattr(exc, "stdout", None)),
                    "stderr": captured_text(getattr(exc, "stderr", None)),
                },
            )
            fail("managed_plugin_handoff", policy_artifact, f"plugin sidecar policy enable failed: {exc}")
        write_json(
            policy_artifact,
            {"returncode": policy.returncode, "stdout": policy.stdout, "stderr": policy.stderr},
        )
        require(
            policy.returncode == 0
            and policy_path.is_file()
            and read_json_file(policy_path).get("state") == "enabled",
            "managed_plugin_handoff",
            policy_artifact,
            "plugin sidecar policy enable failed",
        )
        status_after = {
            "jsonrpc": "2.0",
            "id": "status_after_repair",
            "method": "resources/read",
            "params": {"uri": STATUS_URI, "project": str(project)},
        }
        status = stdio_status_command(
            ["node", str(launcher)],
            artifact,
            timeout_secs,
            project,
            layer="managed_plugin_handoff",
            cwd=project,
            env={
                **os.environ,
                "CODESTORY_CLI": "",
                "CODESTORY_CACHE_ROOT": str(cache_root),
                "CODESTORY_PLUGIN_RELEASE_DIR": str(release_dir),
                "CODESTORY_PLUGIN_SIDECAR_POLICY_PATH": str(policy_path),
                "PLUGIN_DATA": str(plugin_data),
            },
            extra_requests=[
                {
                    "jsonrpc": "2.0",
                    "id": "repair",
                    "method": "tools/call",
                    "params": {
                        "name": "sidecar_setup",
                        "arguments": {"project": str(project), "action": "repair"},
                    },
                },
                status_after,
            ],
            cleanup_status_workers=True,
        )
        require_managed_plugin_handoff(status, artifact, expected_version, archive_cli)
        return status


def stdio_status_command(
    command: list[str],
    artifact: Path,
    timeout_secs: int,
    project: Path,
    layer: str = "serve_stdio",
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    extra_requests: list[dict] | None = None,
    cleanup_status_workers: bool = False,
) -> dict:
    stderr_path = artifact.with_suffix(artifact.suffix + ".stderr.txt")
    process = subprocess.Popen(
        command,
        text=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=cwd,
        env=env,
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
        start_new_session=os.name != "nt",
    )
    requests = [
        {"jsonrpc": "2.0", "id": "tools", "method": "tools/list"},
        {"jsonrpc": "2.0", "id": "resources", "method": "resources/list"},
        {
            "jsonrpc": "2.0",
            "id": "status",
            "method": "resources/read",
            "params": {"uri": STATUS_URI, "project": str(project)},
        },
    ]
    requests.extend(extra_requests or [])
    transcript: list[dict] = []
    stdout_lines: list[str] = []
    stderr_lines: list[str] = []
    stdout_queue: queue.Queue[str | None] = queue.Queue()
    assert process.stdin is not None
    assert process.stdout is not None
    assert process.stderr is not None
    stdout_thread = threading.Thread(target=stream_lines, args=(process.stdout, stdout_lines, stdout_queue), daemon=True)
    stderr_thread = threading.Thread(target=stream_lines, args=(process.stderr, stderr_lines), daemon=True)
    stdout_thread.start()
    stderr_thread.start()
    responses: list[dict] = []
    process_terminated = False
    try:
        for request in requests:
            entry = {"request": request}
            transcript.append(entry)
            process.stdin.write(json.dumps(request) + "\n")
            process.stdin.flush()
            try:
                line = read_stdio_line(stdout_queue, timeout_secs)
            except subprocess.TimeoutExpired:
                entry["timed_out"] = True
                terminate_process_tree(process)
                process_terminated = True
                stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
                write_stdio_artifact(
                    artifact,
                    transcript,
                    "".join(stdout_lines),
                    stderr_path,
                    {"timed_out": True, "timeout_secs": timeout_secs},
                )
                fail(layer, artifact, f"stdio MCP request timed out after {timeout_secs}s")
            if line is None:
                terminate_process_tree(process)
                process_terminated = True
                stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
                write_stdio_artifact(artifact, transcript, "".join(stdout_lines), stderr_path)
                fail(layer, artifact, "stdio MCP server closed before responding")
            try:
                response = json.loads(line)
            except json.JSONDecodeError as exc:
                entry["invalid_line"] = line
                terminate_process_tree(process)
                process_terminated = True
                stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
                write_stdio_artifact(
                    artifact,
                    transcript,
                    "".join(stdout_lines),
                    stderr_path,
                    {"invalid_line": line},
                )
                fail(layer, artifact, f"stdio MCP server emitted invalid JSON: {exc}")
            entry["response"] = response
            responses.append(response)
    finally:
        if not process_terminated:
            terminate_process_tree(process)
            process_terminated = True
        worker_pids: set[int] = set()
        for response in responses:
            if not isinstance(response, dict):
                continue
            if response.get("id") == "repair":
                worker_pid = response.get("result", {}).get("structuredContent", {}).get("pid")
                if isinstance(worker_pid, int):
                    worker_pids.add(worker_pid)
            if cleanup_status_workers:
                status = status_from_resource_response(response)
                if isinstance(status, dict):
                    worker_pids.update(running_status_worker_pids(status))
        for worker_pid in worker_pids:
            terminate_worker_pid(worker_pid)
        try:
            process.stdin.close()
        except OSError:
            pass
        try:
            process.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            process.kill()
        stdout_thread.join(timeout=0.2)
        stderr_thread.join(timeout=0.2)

    stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
    responses_by_id = {response.get("id"): response for response in responses if isinstance(response, dict)}
    tools = responses_by_id.get("tools")
    resources = responses_by_id.get("resources")
    status_response = responses_by_id.get("status")
    payload = {
        "tools": tools,
        "resources": resources,
        "status_response": status_response,
        "transcript": transcript,
        "stdout": "".join(stdout_lines),
    }
    write_json(artifact, payload)
    require(isinstance(tools, dict), layer, artifact, "stdio MCP server did not return tools/list response")
    require(isinstance(resources, dict), layer, artifact, "stdio MCP server did not return resources/list response")
    require(isinstance(status_response, dict), layer, artifact, "stdio MCP server did not return status response")
    if "error" in tools:
        fail(layer, artifact, f"tools/list failed: {tools['error']}")
    if "error" in resources:
        fail(layer, artifact, f"resources/list failed: {resources['error']}")
    if "error" in status_response:
        fail(layer, artifact, f"status resource failed: {status_response['error']}")

    listed_resources = resources.get("result", {}).get("resources", [])
    observed_uris = sorted(
        item.get("uri")
        for item in listed_resources
        if isinstance(item, dict) and isinstance(item.get("uri"), str)
    )
    missing_uris = [uri for uri in SERVER_RESOURCE_URIS if uri not in observed_uris]
    visibility = {
        "required": list(SERVER_RESOURCE_URIS),
        "observed": observed_uris,
        "missing": missing_uris,
        "available": not missing_uris,
    }
    payload["server_advertised_mcp_resources"] = visibility
    write_json(artifact, payload)

    contents = status_response.get("result", {}).get("contents", [])
    content = next((item for item in contents if item.get("uri") == STATUS_URI), None)
    require(content is not None, layer, artifact, "status response missing codestory://status")
    status = json.loads(content.get("text", "{}"))
    payload["status"] = status
    write_json(artifact, payload)
    require(
        not missing_uris,
        layer,
        artifact,
        f"resources/list missing server-advertised CodeStory resources: {', '.join(missing_uris)}",
    )
    return status


def require_stdio_shape(status: dict, artifact: Path, expected_version: str, layer: str = "serve_stdio") -> None:
    require(
        status.get("server_version") == expected_version,
        layer,
        artifact,
        f"status server_version is {status.get('server_version')!r}, expected {expected_version!r}",
    )
    require(
        status.get("cli_version") == expected_version,
        layer,
        artifact,
        f"status cli_version is {status.get('cli_version')!r}, expected {expected_version!r}",
    )
    require(
        isinstance(status.get("server_executable"), str) and len(status.get("server_executable", "")) > 0,
        layer,
        artifact,
        "status missing server_executable",
    )
    require(
        isinstance(status.get("server_executable_sha256"), str)
        and len(status.get("server_executable_sha256", "")) == 64,
        layer,
        artifact,
        "status missing server_executable_sha256",
    )
    require(
        isinstance(status.get("sidecar_contract_version"), int),
        layer,
        artifact,
        "status missing sidecar_contract_version",
    )
    plugin_runtime = status.get("plugin_runtime")
    require(isinstance(plugin_runtime, dict), layer, artifact, "status missing plugin_runtime")
    require(isinstance(status.get("allowed_surfaces"), dict), layer, artifact, "status missing allowed_surfaces")


def require_stdio_ready(status: dict, artifact: Path, expected_version: str) -> None:
    require_stdio_shape(status, artifact, expected_version)
    surfaces = status.get("allowed_surfaces", {})
    for name in ["packet", "search", "context"]:
        allowed = surfaces.get(name, {}).get("allowed")
        require(allowed is True, "serve_stdio", artifact, f"allowed_surfaces.{name}.allowed is {allowed!r}")


def require_plugin_provenance(
    status: dict,
    artifact: Path,
    expected_version: str,
    layer: str,
) -> None:
    require_stdio_shape(status, artifact, expected_version, layer)
    plugin_runtime = status.get("plugin_runtime")
    require(isinstance(plugin_runtime, dict), layer, artifact, "status missing plugin_runtime")
    require(
        plugin_runtime.get("plugin_version") == expected_version,
        layer,
        artifact,
        f"plugin_runtime.plugin_version is {plugin_runtime.get('plugin_version')!r}, expected {expected_version!r}",
    )
    require(
        plugin_runtime.get("cli_source") == "managed",
        layer,
        artifact,
        f"plugin_runtime.cli_source is {plugin_runtime.get('cli_source')!r}, expected 'managed'",
    )
    require(
        plugin_runtime.get("build_source") == "github_release",
        layer,
        artifact,
        f"plugin_runtime.build_source is {plugin_runtime.get('build_source')!r}, expected 'github_release'",
    )
    require(
        plugin_runtime.get("repo_ref") == f"v{expected_version}",
        layer,
        artifact,
        f"plugin_runtime.repo_ref is {plugin_runtime.get('repo_ref')!r}, expected 'v{expected_version}'",
    )
    require(
        plugin_runtime.get("cli_version") == expected_version,
        layer,
        artifact,
        f"plugin_runtime.cli_version is {plugin_runtime.get('cli_version')!r}, expected {expected_version!r}",
    )
    require(
        isinstance(plugin_runtime.get("plugin_root"), str) and len(plugin_runtime.get("plugin_root", "")) > 0,
        layer,
        artifact,
        "plugin_runtime missing plugin_root",
    )


def require_plugin_stdio_ready(status: dict, artifact: Path, expected_version: str) -> None:
    require_stdio_ready(status, artifact, expected_version)
    require_plugin_provenance(status, artifact, expected_version, "plugin_stdio")


def transcript_response(artifact: Path, request_id: str) -> dict | None:
    payload = read_json_file(artifact)
    if not isinstance(payload, dict):
        return None
    for entry in payload.get("transcript", []):
        if isinstance(entry, dict) and entry.get("request", {}).get("id") == request_id:
            response = entry.get("response")
            return response if isinstance(response, dict) else None
    return None


def status_from_resource_response(response: dict) -> dict | None:
    contents = response.get("result", {}).get("contents", [])
    content = next(
        (item for item in contents if isinstance(item, dict) and item.get("uri") == STATUS_URI),
        None,
    )
    if not isinstance(content, dict):
        return None
    try:
        status = json.loads(content.get("text", ""))
    except json.JSONDecodeError:
        return None
    return status if isinstance(status, dict) else None


def require_managed_plugin_handoff(
    status: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> None:
    require_plugin_provenance(status, artifact, expected_version, "managed_plugin_handoff")
    plugin_runtime = status["plugin_runtime"]
    managed_binary = Path(plugin_runtime.get("managed_binary_path", ""))
    server_executable = Path(status.get("server_executable", ""))
    require(
        managed_binary.is_file()
        and server_executable.is_file()
        and managed_binary.samefile(server_executable),
        "managed_plugin_handoff",
        artifact,
        "managed_binary_path does not identify the executable serving MCP",
    )
    archive_sha256 = sha256_file(archive_cli)
    server_sha256 = sha256_file(server_executable)
    require(
        archive_sha256 == server_sha256 == status.get("server_executable_sha256"),
        "managed_plugin_handoff",
        artifact,
        "managed MCP executable does not match the packaged archive binary",
    )
    require(
        status.get("allowed_surfaces", {}).get("ground", {}).get("allowed") is True,
        "managed_plugin_handoff",
        artifact,
        "managed plugin status did not allow local ground",
    )
    repair_response = transcript_response(artifact, "repair") or {}
    repair = repair_response.get("result", {}).get("structuredContent")
    require(isinstance(repair, dict), "managed_plugin_handoff", artifact, "repair response missing structuredContent")
    repair_status = repair.get("status")
    require(
        repair_status in {"started", "already_running"},
        "managed_plugin_handoff",
        artifact,
        f"repair status is {repair_status!r}, expected started or already_running",
    )
    if repair_status == "started":
        require(
            isinstance(repair.get("attempt_id"), str)
            and isinstance(repair.get("pid"), int)
            and repair.get("reservation_published") is True,
            "managed_plugin_handoff",
            artifact,
            "started repair did not publish an attempt reservation",
        )
    repair_setup = repair.get("sidecar_setup") or {}
    attempt_id = repair.get("attempt_id") or (repair_setup.get("active_repair") or {}).get("attempt_id")
    require(
        isinstance(attempt_id, str) and len(attempt_id) > 0,
        "managed_plugin_handoff",
        artifact,
        "repair response did not identify its attempt",
    )
    next_calls = repair.get("recommended_next_calls", [])
    require(
        any(
            isinstance(call, dict)
            and call.get("method") == "tools/call"
            and call.get("tool") == "status"
            and call.get("arguments", {}).get("project")
            for call in next_calls
        ),
        "managed_plugin_handoff",
        artifact,
        "repair response did not point back to project-scoped status",
    )
    status_after_response = transcript_response(artifact, "status_after_repair") or {}
    status_after = status_from_resource_response(status_after_response)
    require(
        isinstance(status_after, dict),
        "managed_plugin_handoff",
        artifact,
        "status read after repair handoff failed",
    )
    require_plugin_provenance(status_after, artifact, expected_version, "managed_plugin_handoff")
    setup = status_after.get("sidecar_setup", {})
    observed_attempts = {
        (setup.get("active_repair") or {}).get("attempt_id"),
        (setup.get("last_worker_result") or {}).get("attempt_id"),
    }
    require(
        attempt_id in observed_attempts,
        "managed_plugin_handoff",
        artifact,
        f"repair attempt {attempt_id!r} was not observable in post-handoff status",
    )


def run_gate(args: argparse.Namespace) -> None:
    archive = Path(args.archive).resolve()
    project = Path(args.project).resolve()
    out_dir = Path(args.out_dir).resolve()
    shutil.rmtree(out_dir, ignore_errors=True)
    out_dir.mkdir(parents=True, exist_ok=True)
    checksum_artifact = out_dir / "archive-checksum.json"
    if args.checksum_file:
        verify_archive_checksum(archive, Path(args.checksum_file).resolve(), checksum_artifact)

    with (
        tempfile.TemporaryDirectory(prefix="codestory-packaged-agent-proof-", dir=out_dir) as temp,
        contextlib.ExitStack() as cleanup_stack,
    ):
        unpacked = Path(temp) / "unpacked"
        unpacked.mkdir()
        unpack_archive(archive, unpacked)
        cli = find_cli(unpacked)
        plugin_skill = find_plugin_skill(unpacked)
        cache_root = Path(tempfile.mkdtemp(prefix="codestory-packaged-proof-cache-"))
        cleanup_stack.callback(cleanup_proof_cache, cli, project, cache_root)
        proof_env = {**os.environ, "CODESTORY_CACHE_ROOT": str(cache_root)}
        stdio_cache_root = Path(tempfile.mkdtemp(prefix="codestory-packaged-stdio-cache-"))
        cleanup_stack.callback(cleanup_proof_cache, cli, project, stdio_cache_root)
        stdio_env = {**os.environ, "CODESTORY_CACHE_ROOT": str(stdio_cache_root)}

        summary = {
            "archive": str(archive),
            "cli": str(cli),
            "plugin_skill": str(plugin_skill),
            "project": str(project),
            "artifacts": {},
        }
        if args.checksum_file:
            summary["artifacts"]["checksum"] = str(checksum_artifact)
        write_json(out_dir / "summary.json", summary)

        version_artifact = out_dir / "version.txt"
        run_command(cli, "version", ["--version"], version_artifact, args.timeout_secs, parse_json=False)
        require_version(version_artifact.read_text(encoding="utf-8"), args.expected_version, archive, version_artifact)
        summary["artifacts"]["version"] = str(version_artifact)
        write_json(out_dir / "summary.json", summary)

        help_artifact = out_dir / "help.txt"
        run_command(cli, "help", ["--help"], help_artifact, args.timeout_secs, parse_json=False)
        require_help(help_artifact.read_text(encoding="utf-8"), help_artifact)
        summary["artifacts"]["help"] = str(help_artifact)
        write_json(out_dir / "summary.json", summary)

        stdio_artifact = out_dir / "serve-stdio-status.json"
        if args.version_only or args.managed_plugin_handoff:
            stdio_status_payload = stdio_status(
                cli,
                project,
                stdio_artifact,
                args.timeout_secs,
                stdio_env,
                cleanup_status_workers=True,
            )
            require_stdio_shape(stdio_status_payload, stdio_artifact, args.expected_version)
            summary["artifacts"]["serve_stdio"] = str(stdio_artifact)
            write_json(out_dir / "summary.json", summary)

        if args.version_only:
            return

        if args.managed_plugin_handoff:
            local_ready_artifact = out_dir / "managed-local-ready.json"
            run_command(
                cli,
                "managed_local_ready",
                [
                    "ready",
                    "--goal",
                    "local",
                    "--repair",
                    "--project",
                    str(project),
                    "--format",
                    "json",
                    "--output-file",
                    str(local_ready_artifact),
                ],
                local_ready_artifact,
                args.timeout_secs,
                env=proof_env,
            )
            summary["artifacts"]["managed_local_ready"] = str(local_ready_artifact)

            local_ground_artifact = out_dir / "managed-local-ground.json"
            ground = run_command(
                cli,
                "managed_local_ground",
                [
                    "ground",
                    "--project",
                    str(project),
                    "--refresh",
                    "none",
                    "--format",
                    "json",
                    "--output-file",
                    str(local_ground_artifact),
                ],
                local_ground_artifact,
                args.timeout_secs,
                env=proof_env,
            )
            require(
                isinstance(ground, dict) and isinstance(ground.get("stats"), dict),
                "managed_local_ground",
                local_ground_artifact,
                "managed local ground output is missing repository stats",
            )
            summary["artifacts"]["managed_local_ground"] = str(local_ground_artifact)

            plugin_artifact = out_dir / "managed-plugin-handoff.json"
            plugin_status = plugin_stdio_handoff(
                Path(args.plugin_root).resolve(),
                archive.parent,
                project,
                cache_root,
                plugin_artifact,
                args.timeout_secs,
                args.expected_version,
                cli,
            )
            summary["artifacts"]["managed_plugin_handoff"] = str(plugin_artifact)
            write_json(out_dir / "summary.json", summary)
            return

        local_bootstrap_artifact = out_dir / "local-retrieval-bootstrap.json"
        run_command(
            cli,
            "local_retrieval_bootstrap",
            [
                "retrieval",
                "bootstrap",
                "--project",
                str(project),
                "--profile",
                "local",
                "--format",
                "json",
                "--output-file",
                str(local_bootstrap_artifact),
            ],
            local_bootstrap_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        summary["artifacts"]["local_retrieval_bootstrap"] = str(local_bootstrap_artifact)
        write_json(out_dir / "summary.json", summary)

        local_index_artifact = out_dir / "local-retrieval-index.json"
        local_index = run_command(
            cli,
            "local_retrieval_index",
            [
                "retrieval",
                "index",
                "--project",
                str(project),
                "--profile",
                "local",
                "--refresh",
                "full",
                "--format",
                "json",
                "--output-file",
                str(local_index_artifact),
            ],
            local_index_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_retrieval_index_ready(local_index, "local_retrieval_index", local_index_artifact)
        summary["artifacts"]["local_retrieval_index"] = str(local_index_artifact)
        write_json(out_dir / "summary.json", summary)

        local_status_artifact = out_dir / "local-retrieval-status.json"
        local_status = run_command(
            cli,
            "local_retrieval_status",
            [
                "retrieval",
                "status",
                "--project",
                str(project),
                "--profile",
                "local",
                "--format",
                "json",
                "--output-file",
                str(local_status_artifact),
            ],
            local_status_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_retrieval_full(local_status, "local_retrieval_status", local_status_artifact)
        summary["artifacts"]["local_retrieval_status"] = str(local_status_artifact)
        write_json(out_dir / "summary.json", summary)

        ready_artifact = out_dir / "ready.json"
        ready = run_command(
            cli,
            "ready",
            [
                "ready",
                "--goal",
                "agent",
                "--repair",
                "--project",
                str(project),
                "--format",
                "json",
                "--output-file",
                str(ready_artifact),
            ],
            ready_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_agent_ready(ready, "ready", ready_artifact)
        summary["artifacts"]["ready"] = str(ready_artifact)
        write_json(out_dir / "summary.json", summary)

        doctor_artifact = out_dir / "doctor.json"
        doctor = run_command(
            cli,
            "doctor",
            ["doctor", "--project", str(project), "--format", "json", "--output-file", str(doctor_artifact)],
            doctor_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_retrieval_full(doctor, "doctor", doctor_artifact)
        summary["artifacts"]["doctor"] = str(doctor_artifact)
        write_json(out_dir / "summary.json", summary)

        status_artifact = out_dir / "retrieval-status.json"
        status = run_command(
            cli,
            "retrieval_status",
            [
                "retrieval",
                "status",
                "--project",
                str(project),
                "--format",
                "json",
                "--output-file",
                str(status_artifact),
            ],
            status_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_retrieval_full(status, "retrieval_status", status_artifact)
        summary["artifacts"]["retrieval_status"] = str(status_artifact)
        write_json(out_dir / "summary.json", summary)

        search_artifact = out_dir / "search.json"
        search = run_command(
            cli,
            "search",
            [
                "search",
                "--project",
                str(project),
                "--query",
                args.query,
                "--why",
                "--format",
                "json",
                "--output-file",
                str(search_artifact),
            ],
            search_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_search_full(search, search_artifact)
        summary["artifacts"]["search"] = str(search_artifact)
        write_json(out_dir / "summary.json", summary)

        context_artifact = out_dir / "context.json"
        context = run_command(
            cli,
            "context",
            [
                "context",
                "--project",
                str(project),
                "--query",
                args.context_query,
                "--format",
                "json",
                "--output-file",
                str(context_artifact),
            ],
            context_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_context_ready(context, context_artifact)
        summary["artifacts"]["context"] = str(context_artifact)
        write_json(out_dir / "summary.json", summary)

        packet_artifact = out_dir / "packet.json"
        packet = run_command(
            cli,
            "packet",
            [
                "packet",
                "--project",
                str(project),
                "--question",
                args.question,
                "--budget",
                "compact",
                "--format",
                "json",
                "--output-file",
                str(packet_artifact),
            ],
            packet_artifact,
            args.timeout_secs,
            env=proof_env,
        )
        require_packet_ready(packet, packet_artifact)
        summary["artifacts"]["packet"] = str(packet_artifact)
        write_json(out_dir / "summary.json", summary)

        stdio_attempts = []
        try:
            stdio_status_payload = stdio_status(cli, project, stdio_artifact, args.timeout_secs, proof_env)
            stdio_attempts.append(stdio_status_payload)
            require_stdio_shape(stdio_status_payload, stdio_artifact, args.expected_version)
            allowed = stdio_status_payload.get("allowed_surfaces", {})
            if not all(allowed.get(name, {}).get("allowed") is True for name in ("packet", "search", "context")):
                shutil.copy2(stdio_artifact, out_dir / "serve-stdio-status-initial.json")
                stdio_status_payload = stdio_status(cli, project, stdio_artifact, args.timeout_secs, proof_env)
                stdio_attempts.append(stdio_status_payload)
            require_stdio_ready(stdio_status_payload, stdio_artifact, args.expected_version)
        finally:
            for status_attempt in stdio_attempts:
                terminate_status_workers(status_attempt)
        summary["artifacts"]["serve_stdio"] = str(stdio_artifact)
        write_json(out_dir / "summary.json", summary)

        if args.plugin_root:
            plugin_stdio_artifact = out_dir / "plugin-stdio-status.json"
            plugin_status = plugin_stdio_status(
                Path(args.plugin_root).resolve(),
                archive.parent,
                project,
                plugin_stdio_artifact,
                args.timeout_secs,
                args.expected_version,
                cache_root,
            )
            require_plugin_stdio_ready(plugin_status, plugin_stdio_artifact, args.expected_version)
            summary["artifacts"]["plugin_stdio"] = str(plugin_stdio_artifact)
            write_json(out_dir / "summary.json", summary)

    print(f"packaged agent proof passed; artifacts={out_dir}")


def write_fake_cli(path: Path) -> None:
    fake = path / "fake_cli.py"
    fake.write_text(
        textwrap.dedent(
            r'''
            import hashlib
            import json
            import os
            import sys
            import time

            def emit(value):
                if "--output-file" in sys.argv:
                    out = sys.argv[sys.argv.index("--output-file") + 1]
                    open(out, "w", encoding="utf-8").write(json.dumps(value))
                else:
                    print(json.dumps(value))

            fail = os.environ.get("CODESTORY_FAKE_FAIL_LAYER")
            if "--version" in sys.argv:
                print("codestory-cli 9.9.9")
                raise SystemExit(0)
            if "--help" in sys.argv:
                print("Usage: codestory-cli [OPTIONS] <COMMAND>")
                raise SystemExit(0)
            layer = sys.argv[1]
            if layer == "retrieval" and len(sys.argv) > 2:
                layer = "retrieval_" + sys.argv[2].replace("-", "_")
            if fail == f"{layer}_stderr":
                print("forced stderr failure", file=sys.stderr)
                raise SystemExit(3)
            if fail == layer:
                print("forced failure")
                raise SystemExit(2)
            if layer == "ready":
                emit({"verdicts": [{
                    "goal": "agent_packet_search",
                    "status": "ready",
                    "summary": "ready",
                    "minimum_next": [],
                    "full_repair": [],
                }]})
            elif layer == "doctor":
                emit({"retrieval_mode": "full"})
            elif layer == "retrieval_bootstrap":
                emit({
                    "cache_root": os.environ.get("CODESTORY_CACHE_ROOT"),
                    "project_status": {"retrieval_mode": "unavailable"},
                })
            elif layer == "retrieval_index":
                emit({"zoekt_stubbed": False, "qdrant_stubbed": False, "scip_stubbed": False})
            elif layer == "retrieval_status":
                emit({"retrieval_mode": "full"})
            elif layer == "search":
                emit({"retrieval_shadow": {"retrieval_mode": "full"}, "indexed_symbol_hits": [{"node_id": "1"}]})
            elif layer == "context":
                if fail == "context_weak":
                    emit({"retrieval_trace": {"resolved_profile": "investigate"}})
                elif fail == "context_fallback":
                    emit({
                        "context": {
                            "retrieval_version": "sidecar",
                            "retrieval_trace": {
                                "steps": [{}],
                                "retrieval_shadow": {"retrieval_mode": "fallback"},
                            },
                        },
                    })
                else:
                    emit({
                        "context": {
                            "retrieval_version": "sidecar",
                            "retrieval_trace": {
                                "resolved_profile": "investigate",
                                "steps": [{}],
                                "retrieval_shadow": {"retrieval_mode": "full"},
                            },
                        },
                    })
            elif layer == "packet":
                if fail == "packet_weak":
                    emit({"sufficiency": {"status": "supported"}, "answer": {"retrieval_version": "fallback"}, "retrieval_trace_summary": {}})
                else:
                    emit({"sufficiency": {"status": "sufficient"}, "answer": {"retrieval_version": "sidecar"}, "retrieval_trace_summary": {}})
            elif layer == "ground":
                emit({
                    "cache_root": os.environ.get("CODESTORY_CACHE_ROOT"),
                    "root": os.getcwd(),
                    "stats": {"file_count": 1, "node_count": 1},
                })
            elif layer == "serve":
                marker = None
                repair_attempt = None
                if fail in {"serve_first_blocked", "serve_first_blocked_then_timeout"}:
                    try:
                        project = sys.argv[sys.argv.index("--project") + 1]
                        marker = os.path.join(project, ".fake-serve-first-blocked-seen")
                    except (ValueError, IndexError):
                        marker = os.path.join(os.getcwd(), ".fake-serve-first-blocked-seen")
                for line in sys.stdin:
                    request = json.loads(line)
                    if fail == "serve_timeout":
                        time.sleep(60)
                        continue
                    if request.get("method") == "tools/list":
                        result = {"tools": [{"name": "packet"}, {"name": "search"}, {"name": "context"}, {"name": "sidecar_setup"}]}
                    elif request.get("method") == "resources/list":
                        resources = [{"uri": "codestory://status", "name": "CodeStory runtime status"}]
                        if fail != "resources_hidden":
                            resources.append({"uri": "codestory://agent-guide", "name": "CodeStory agent guide"})
                        result = {"resources": resources}
                    elif request.get("method") == "tools/call" and request.get("params", {}).get("name") == "sidecar_setup":
                        repair = {
                            "status": "started",
                            "mode": "background",
                            "pid": os.getpid(),
                            "attempt_id": "fake-attempt",
                            "reservation_published": True,
                            "recommended_next_calls": [{
                                "method": "tools/call",
                                "tool": "status",
                                "arguments": {"project": os.getcwd()},
                            }],
                        }
                        repair_attempt = repair["attempt_id"]
                        result = {"content": [{"type": "text", "text": json.dumps(repair)}], "structuredContent": repair}
                    else:
                        if fail == "serve_first_blocked_then_timeout" and marker is not None and os.path.exists(marker):
                            time.sleep(60)
                            continue
                        serve_allowed = True
                        refresh_worker_pid = None
                        if marker is not None and not os.path.exists(marker):
                            refresh_worker_pid = int(os.environ["CODESTORY_FAKE_STATUS_WORKER_PID"])
                            open(marker, "w", encoding="utf-8").write(str(refresh_worker_pid))
                            serve_allowed = False
                        elif marker is not None:
                            refresh_worker_pid = int(open(marker, encoding="utf-8").read())
                            try:
                                os.kill(refresh_worker_pid, 0)
                            except OSError:
                                serve_allowed = False
                        server_executable = os.environ.get("CODESTORY_FAKE_SERVER_EXECUTABLE", sys.argv[0])
                        server_sha256 = hashlib.sha256(open(server_executable, "rb").read()).hexdigest()
                        status = {
                            "cache_root": os.environ.get("CODESTORY_CACHE_ROOT"),
                            "readiness_broker": {
                                "operations": ([{
                                    "operation_kind": "local_graph_refresh",
                                    "pid": refresh_worker_pid,
                                    "status": "running",
                                }] if refresh_worker_pid else []),
                            },
                            "server_version": "9.9.9",
                            "cli_version": "9.9.9",
                            "server_executable": server_executable,
                            "server_executable_sha256": server_sha256,
                            "sidecar_contract_version": 1,
                            "plugin_runtime": {
                                "cli_source": "managed" if os.environ.get("CODESTORY_FAKE_PLUGIN_MANAGED") == "1" else "direct_cli_launch",
                                "plugin_version": "9.9.9",
                                "build_source": "github_release",
                                "repo_ref": "v9.9.9",
                                "cli_version": "9.9.9",
                                "plugin_root": os.getcwd(),
                                "managed_binary_path": server_executable,
                            },
                            "sidecar_setup": {
                                "active_repair": {"attempt_id": repair_attempt} if repair_attempt else None,
                                "last_worker_result": None,
                            },
                            "allowed_surfaces": {
                                "ground": {"allowed": True},
                                "packet": {"allowed": serve_allowed},
                                "search": {"allowed": serve_allowed},
                                "context": {"allowed": serve_allowed},
                            },
                        }
                        result = {"contents": [{"uri": "codestory://status", "mimeType": "application/json", "text": json.dumps(status)}]}
                    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
            else:
                raise SystemExit(f"unknown fake layer: {layer}")
            '''
        ).lstrip(),
        encoding="utf-8",
    )
    if os.name == "nt":
        wrapper = path / "codestory-cli.cmd"
        wrapper.write_text(f'@echo off\r\n"{sys.executable}" "%~dp0fake_cli.py" %*\r\n', encoding="utf-8")
    else:
        wrapper = path / "codestory-cli"
        wrapper.write_text(f"#!{sys.executable}\nimport runpy\nrunpy.run_path({str(fake)!r}, run_name='__main__')\n", encoding="utf-8")
        wrapper.chmod(wrapper.stat().st_mode | stat.S_IXUSR)


def write_fake_plugin_launcher(plugin_root: Path) -> None:
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    launcher.parent.mkdir(parents=True)
    launcher.write_text(
        textwrap.dedent(
            r'''
            const { spawn } = require('child_process');
            const fs = require('fs');
            const path = require('path');

            if (process.argv[2] === 'sidecar-policy') {
              if (process.env.CODESTORY_FAKE_PLUGIN_POLICY_TIMEOUT === '1') {
                process.stdout.write('policy stdout before timeout\n');
                process.stderr.write('policy stderr before timeout\n');
                Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60_000);
              }
              const policyIndex = process.argv.indexOf('--policy-file');
              if (policyIndex >= 0 && process.argv[policyIndex + 1]) {
                fs.writeFileSync(process.argv[policyIndex + 1], JSON.stringify({ state: 'enabled' }));
              }
              process.exit(0);
            }

            let cli = process.env.CODESTORY_FAKE_PLUGIN_CLI;
            if (!cli) {
              process.stderr.write('CODESTORY_FAKE_PLUGIN_CLI is required\n');
              process.exit(2);
            }
            if (process.env.CODESTORY_FAKE_PLUGIN_MANAGED === '1') {
              const managedDir = path.join(process.env.PLUGIN_DATA, 'codestory-cli', '9.9.9', 'bin');
              fs.mkdirSync(managedDir, { recursive: true });
              const sourceDir = path.dirname(cli);
              const managedCli = path.join(managedDir, path.basename(cli));
              fs.copyFileSync(cli, managedCli);
              fs.copyFileSync(path.join(sourceDir, 'fake_cli.py'), path.join(managedDir, 'fake_cli.py'));
              cli = managedCli;
            }

            const child = spawn(
              cli,
              ['serve', '--stdio', '--refresh', 'none', '--project', process.cwd()],
              {
                stdio: 'inherit',
                shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(cli),
                env: {
                  ...process.env,
                  CODESTORY_FAKE_FAIL_LAYER: process.env.CODESTORY_FAKE_PLUGIN_HIDE_RESOURCES === '1'
                    ? 'resources_hidden'
                    : process.env.CODESTORY_FAKE_FAIL_LAYER || '',
                  CODESTORY_FAKE_SERVER_EXECUTABLE: cli,
                },
              },
            );
            child.on('exit', (code, signal) => {
              if (signal) process.kill(process.pid, signal);
              process.exit(code || 0);
            });
            child.on('error', (error) => {
              process.stderr.write(`${error.message}\n`);
              process.exit(1);
            });
            '''
        ).lstrip(),
        encoding="utf-8",
    )


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-self-test-") as temp:
        root = Path(temp)
        stage = root / "pkg" / "codestory-cli-v9.9.9-test"
        stage.mkdir(parents=True)
        write_fake_cli(stage)
        skill = stage / PLUGIN_SKILL_RELATIVE
        skill.parent.mkdir(parents=True)
        skill.write_text("name: codestory-grounding\n", encoding="utf-8")
        archive = root / "codestory-cli-v9.9.9-test.zip"
        with zipfile.ZipFile(archive, "w") as handle:
            for path in stage.rglob("*"):
                handle.write(path, path.relative_to(stage.parent).as_posix())
        checksum_file = root / "SHA256SUMS.txt"
        checksum_file.write_text(f"{sha256_file(archive)}  {archive.name}\n", encoding="utf-8")
        project = root / "repo"
        project.mkdir()
        out_dir = root / "out"
        args = argparse.Namespace(
            archive=str(archive),
            project=str(project),
            out_dir=str(out_dir),
            query=DEFAULT_QUERY,
            context_query=DEFAULT_QUERY,
            question=DEFAULT_QUESTION,
            expected_version="9.9.9",
            checksum_file=str(checksum_file),
            plugin_root=None,
            timeout_secs=30,
            version_only=False,
            managed_plugin_handoff=False,
        )
        run_gate(args)
        assert (out_dir / "summary.json").is_file()
        stdio_artifact = read_json_file(out_dir / "serve-stdio-status.json")
        full_cache_root = read_json_file(out_dir / "local-retrieval-bootstrap.json")["cache_root"]
        assert Path(full_cache_root).name.startswith("codestory-packaged-proof-cache-")
        assert stdio_artifact["status"]["cache_root"] == full_cache_root
        status_request = next(
            entry["request"]
            for entry in stdio_artifact["transcript"]
            if entry["request"].get("id") == "status"
        )
        assert status_request["params"]["project"] == str(project.resolve())

        version_only_out = root / "version-only-out"
        version_only_args = argparse.Namespace(**vars(args))
        version_only_args.out_dir = str(version_only_out)
        version_only_args.version_only = True
        run_gate(version_only_args)
        version_only_summary = read_json_file(version_only_out / "summary.json")
        assert version_only_summary["artifacts"].keys() == {"checksum", "version", "help", "serve_stdio"}
        assert not (version_only_out / "local-retrieval-bootstrap.json").exists()
        version_only_status = read_json_file(version_only_out / "serve-stdio-status.json")["status"]
        assert Path(version_only_status["cache_root"]).name.startswith("codestory-packaged-stdio-cache-")
        assert version_only_status["cache_root"] != full_cache_root

        bad_checksum = root / "BAD_SHA256SUMS.txt"
        bad_checksum.write_text(f"{'0' * 64}  {archive.name}\n", encoding="utf-8")
        bad_checksum_args = argparse.Namespace(**vars(version_only_args))
        bad_checksum_args.out_dir = str(root / "bad-checksum-out")
        bad_checksum_args.checksum_file = str(bad_checksum)
        try:
            run_gate(bad_checksum_args)
        except GateFailure as exc:
            assert exc.layer == "checksum"
            assert exc.artifact.name == "archive-checksum.json"
        else:
            raise AssertionError("mismatched packaged checksum should fail the gate")

        stale_out = root / "stale-out"
        stale_out.mkdir()
        stale_file = stale_out / "stale-plugin-stdio-status.json"
        stale_file.write_text("stale", encoding="utf-8")
        stale_args = argparse.Namespace(**vars(version_only_args))
        stale_args.out_dir = str(stale_out)
        run_gate(stale_args)
        assert not stale_file.exists()
        assert (stale_out / "summary.json").is_file()

        delayed_stdio_project = root / "repo-delayed-stdio"
        delayed_stdio_project.mkdir()
        delayed_stdio_out = root / "delayed-stdio-out"
        delayed_stdio_args = argparse.Namespace(**vars(args))
        delayed_stdio_args.project = str(delayed_stdio_project)
        delayed_stdio_args.out_dir = str(delayed_stdio_out)
        delayed_status_worker = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=(
                subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS
                if os.name == "nt"
                else 0
            ),
            start_new_session=os.name != "nt",
        )
        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "serve_first_blocked"
        os.environ["CODESTORY_FAKE_STATUS_WORKER_PID"] = str(delayed_status_worker.pid)
        try:
            run_gate(delayed_stdio_args)
            delayed_stdio_status = read_json_file(delayed_stdio_out / "serve-stdio-status.json")
            delayed_status = delayed_stdio_status["status"]
            assert delayed_status["allowed_surfaces"]["packet"]["allowed"] is True
            assert (delayed_stdio_project / ".fake-serve-first-blocked-seen").is_file()
            delayed_status_worker.wait(timeout=5)
            assert delayed_status_worker.returncode is not None
        finally:
            terminate_worker_pid(delayed_status_worker.pid)
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)
            os.environ.pop("CODESTORY_FAKE_STATUS_WORKER_PID", None)

        retry_timeout_project = root / "repo-retry-timeout"
        retry_timeout_project.mkdir()
        retry_timeout_args = argparse.Namespace(**vars(args))
        retry_timeout_args.project = str(retry_timeout_project)
        retry_timeout_args.out_dir = str(root / "retry-timeout-out")
        retry_timeout_args.timeout_secs = 5
        retry_timeout_worker = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=(
                subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS
                if os.name == "nt"
                else 0
            ),
            start_new_session=os.name != "nt",
        )
        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "serve_first_blocked_then_timeout"
        os.environ["CODESTORY_FAKE_STATUS_WORKER_PID"] = str(retry_timeout_worker.pid)
        try:
            try:
                run_gate(retry_timeout_args)
            except GateFailure as exc:
                assert exc.layer == "serve_stdio"
                assert "serve-stdio-status.json" in str(exc.artifact)
            else:
                raise AssertionError("timed-out stdio readiness retry should fail the gate")
            retry_timeout_worker.wait(timeout=5)
            assert retry_timeout_worker.returncode is not None
        finally:
            terminate_worker_pid(retry_timeout_worker.pid)
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)
            os.environ.pop("CODESTORY_FAKE_STATUS_WORKER_PID", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "search"
        try:
            try:
                run_gate(args)
            except GateFailure as exc:
                assert exc.layer == "search"
                assert "search.json.stdout.txt" in str(exc.artifact)
            else:
                raise AssertionError("forced fake search failure should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

        mismatch_args = argparse.Namespace(**vars(args))
        mismatch_args.expected_version = "9.9.8"
        try:
            run_gate(mismatch_args)
        except GateFailure as exc:
            assert exc.layer == "version"
            assert "version.txt" in str(exc.artifact)
        else:
            raise AssertionError("version mismatch should fail the gate")

        plugin_root = root / "plugin"
        plugin_manifest = plugin_root / ".codex-plugin" / "plugin.json"
        plugin_manifest.parent.mkdir(parents=True)
        plugin_manifest.write_text(json.dumps({"version": "9.9.8"}), encoding="utf-8")
        try:
            require_plugin_manifest_version(plugin_root, "9.9.9")
        except GateFailure as exc:
            assert exc.layer == "plugin_manifest"
            assert exc.artifact == plugin_manifest
        else:
            raise AssertionError("plugin manifest version mismatch should fail the gate")

        plugin_drift_args = argparse.Namespace(**vars(args))
        plugin_drift_args.plugin_root = str(plugin_root)
        try:
            run_gate(plugin_drift_args)
        except GateFailure as exc:
            assert exc.layer == "plugin_manifest"
            assert exc.artifact == plugin_manifest
        else:
            raise AssertionError("plugin-root drift should fail the packaged proof gate")

        plugin_manifest.write_text(json.dumps({"version": "9.9.9"}), encoding="utf-8")
        write_fake_plugin_launcher(plugin_root)
        plugin_hidden_args = argparse.Namespace(**vars(args))
        plugin_hidden_args.plugin_root = str(plugin_root)
        plugin_hidden_args.out_dir = str(root / "plugin-hidden-out")
        os.environ["CODESTORY_FAKE_PLUGIN_HIDE_RESOURCES"] = "1"
        os.environ["CODESTORY_FAKE_PLUGIN_CLI"] = str(stage / ("codestory-cli.cmd" if os.name == "nt" else "codestory-cli"))
        try:
            try:
                run_gate(plugin_hidden_args)
            except GateFailure as exc:
                assert exc.layer == "plugin_stdio"
                assert "plugin-stdio-status.json" in str(exc.artifact)
                artifact = read_json_file(exc.artifact)
                assert artifact["server_advertised_mcp_resources"]["missing"] == ["codestory://agent-guide"]
                assert artifact["status"]["plugin_runtime"]["cli_source"] == "direct_cli_launch"
                hidden_cache_root = read_json_file(
                    Path(plugin_hidden_args.out_dir) / "local-retrieval-bootstrap.json"
                )["cache_root"]
                assert artifact["status"]["cache_root"] == hidden_cache_root
            else:
                raise AssertionError("hidden plugin MCP resources should fail the plugin stdio gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_PLUGIN_HIDE_RESOURCES", None)
            os.environ.pop("CODESTORY_FAKE_PLUGIN_CLI", None)

        managed_args = argparse.Namespace(**vars(args))
        managed_args.plugin_root = str(plugin_root)
        managed_args.out_dir = str(root / "managed-plugin-out")
        managed_args.managed_plugin_handoff = True
        os.environ["CODESTORY_FAKE_PLUGIN_CLI"] = str(
            stage / ("codestory-cli.cmd" if os.name == "nt" else "codestory-cli")
        )
        os.environ["CODESTORY_FAKE_PLUGIN_MANAGED"] = "1"
        try:
            run_gate(managed_args)
            managed_summary = read_json_file(Path(managed_args.out_dir) / "summary.json")
            assert "managed_local_ground" in managed_summary["artifacts"]
            assert "managed_plugin_handoff" in managed_summary["artifacts"]
            managed_artifact = Path(managed_args.out_dir) / "managed-plugin-handoff.json"
            assert transcript_response(managed_artifact, "repair")["result"]["structuredContent"]["status"] == "started"
            managed_status_after = status_from_resource_response(
                transcript_response(managed_artifact, "status_after_repair")
            )
            assert managed_status_after["sidecar_setup"]["active_repair"]["attempt_id"] == "fake-attempt"
            managed_cache_root = read_json_file(Path(managed_args.out_dir) / "managed-local-ground.json")["cache_root"]
            assert Path(managed_cache_root).name.startswith("codestory-packaged-proof-cache-")
            assert managed_status_after["cache_root"] == managed_cache_root
            managed_direct_status = read_json_file(Path(managed_args.out_dir) / "serve-stdio-status.json")["status"]
            assert Path(managed_direct_status["cache_root"]).name.startswith("codestory-packaged-stdio-cache-")
            assert managed_direct_status["cache_root"] != managed_cache_root
        finally:
            os.environ.pop("CODESTORY_FAKE_PLUGIN_MANAGED", None)
            os.environ.pop("CODESTORY_FAKE_PLUGIN_CLI", None)

        policy_timeout_artifact = root / "policy-timeout" / "managed-plugin-handoff.json"
        policy_timeout_artifact.parent.mkdir(parents=True)
        os.environ["CODESTORY_FAKE_PLUGIN_CLI"] = str(
            stage / ("codestory-cli.cmd" if os.name == "nt" else "codestory-cli")
        )
        os.environ["CODESTORY_FAKE_PLUGIN_POLICY_TIMEOUT"] = "1"
        try:
            try:
                plugin_stdio_handoff(
                    plugin_root,
                    archive.parent,
                    project,
                    root / "policy-timeout-cache",
                    policy_timeout_artifact,
                    2,
                    "9.9.9",
                    stage / ("codestory-cli.cmd" if os.name == "nt" else "codestory-cli"),
                )
            except GateFailure as exc:
                assert exc.layer == "managed_plugin_handoff"
                assert exc.artifact.name == "managed-plugin-policy.json"
                policy_timeout = read_json_file(exc.artifact)
                assert "policy stdout before timeout" in policy_timeout["stdout"]
                assert "policy stderr before timeout" in policy_timeout["stderr"]
            else:
                raise AssertionError("timed-out plugin policy enable should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_PLUGIN_POLICY_TIMEOUT", None)
            os.environ.pop("CODESTORY_FAKE_PLUGIN_CLI", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "doctor_stderr"
        try:
            try:
                run_gate(args)
            except GateFailure as exc:
                assert exc.layer == "doctor"
                assert "doctor.json.stderr.txt" in str(exc.artifact)
            else:
                raise AssertionError("stderr fake failure should point at stderr artifact")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "packet_weak"
        try:
            try:
                run_gate(args)
            except GateFailure as exc:
                assert exc.layer == "packet"
                assert "packet.json" in str(exc.artifact)
            else:
                raise AssertionError("weak fake packet should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "context_weak"
        try:
            try:
                run_gate(args)
            except GateFailure as exc:
                assert exc.layer == "context"
                assert "context.json" in str(exc.artifact)
            else:
                raise AssertionError("weak fake context should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "context_fallback"
        try:
            try:
                run_gate(args)
            except GateFailure as exc:
                assert exc.layer == "context"
                assert "context.json" in str(exc.artifact)
            else:
                raise AssertionError("fallback fake context should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "serve_timeout"
        timeout_args = argparse.Namespace(**vars(args))
        timeout_args.timeout_secs = 5
        try:
            try:
                run_gate(timeout_args)
            except GateFailure as exc:
                assert exc.layer == "serve_stdio"
                assert "serve-stdio-status.json" in str(exc.artifact)
            else:
                raise AssertionError("silent fake stdio server should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

        os.environ["CODESTORY_FAKE_FAIL_LAYER"] = "resources_hidden"
        try:
            try:
                run_gate(args)
            except GateFailure as exc:
                assert exc.layer == "serve_stdio"
                assert "serve-stdio-status.json" in str(exc.artifact)
            else:
                raise AssertionError("hidden MCP resources should fail the gate")
        finally:
            os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)

    print("self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Gate releases on packaged full-sidecar agent proof.")
    parser.add_argument("--archive", help="Packaged codestory-cli archive to test.")
    parser.add_argument("--project", default=".", help="Representative repository to prove against.")
    parser.add_argument("--out-dir", default="target/packaged-agent-proof", help="Artifact directory.")
    parser.add_argument("--query", default=DEFAULT_QUERY, help="Search proof query.")
    parser.add_argument("--context-query", default=DEFAULT_QUERY, help="Context proof target query.")
    parser.add_argument("--question", default=DEFAULT_QUESTION, help="Packet proof question.")
    parser.add_argument("--expected-version", help="Expected codestory-cli version in the archive.")
    parser.add_argument("--checksum-file", help="SHA256SUMS file that must contain and match the archive.")
    parser.add_argument("--plugin-root", help="Plugin root to smoke through scripts/codestory-mcp.cjs.")
    parser.add_argument("--timeout-secs", type=int, default=1800, help="Per-layer timeout.")
    parser.add_argument(
        "--version-only",
        action="store_true",
        help="Only run archive, version, help, and stdio status-shape smoke.",
    )
    parser.add_argument(
        "--managed-plugin-handoff",
        action="store_true",
        help="Prove managed plugin provisioning, local ground, and background repair handoff without requiring full sidecars.",
    )
    parser.add_argument("--self-test", action="store_true", help="Run script self-tests.")
    args = parser.parse_args()
    if not args.self_test and not args.archive:
        parser.error("--archive is required unless --self-test is set")
    if not args.self_test and not args.expected_version:
        parser.error("--expected-version is required unless --self-test is set")
    if args.managed_plugin_handoff and not args.plugin_root:
        parser.error("--plugin-root is required with --managed-plugin-handoff")
    if args.managed_plugin_handoff and args.version_only:
        parser.error("--managed-plugin-handoff and --version-only are mutually exclusive")
    return args


def main() -> None:
    args = parse_args()
    if args.self_test:
        self_test()
        return
    try:
        run_gate(args)
    except GateFailure as exc:
        print(f"::error::layer={exc.layer} artifact={exc.artifact} {exc.message}", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
