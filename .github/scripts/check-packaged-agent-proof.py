#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import queue
import signal
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


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def read_json_file(path: Path) -> object:
    return json.loads(path.read_text(encoding="utf-8"))


def run_command(
    cli: Path,
    layer: str,
    args: list[str],
    artifact: Path,
    timeout_secs: int,
    parse_json: bool = True,
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
        )
    except subprocess.TimeoutExpired as exc:
        stdout_path.write_text(exc.stdout or "", encoding="utf-8")
        stderr_path.write_text(exc.stderr or "", encoding="utf-8")
        fail(layer, stdout_path, f"command timed out after {timeout_secs}s: {' '.join(command)}")

    stdout_path.write_text(result.stdout, encoding="utf-8")
    stderr_path.write_text(result.stderr, encoding="utf-8")
    if result.returncode != 0:
        artifact_path = stderr_path if result.stderr.strip() else stdout_path
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


def require_retrieval_full(payload: object, layer: str, artifact: Path) -> None:
    mode = payload.get("retrieval_mode") if isinstance(payload, dict) else None
    if mode is None and isinstance(payload, dict):
        mode = payload.get("sidecar_retrieval", {}).get("retrieval_mode")
    require(mode == "full", layer, artifact, f"retrieval_mode is {mode!r}, expected 'full'")


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


def stdio_status(cli: Path, project: Path, artifact: Path, timeout_secs: int) -> dict:
    stderr_path = artifact.with_suffix(artifact.suffix + ".stderr.txt")
    command = [
        str(cli),
        "serve",
        "--stdio",
        "--refresh",
        "none",
        "--project",
        str(project),
    ]
    process = subprocess.Popen(
        command,
        text=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
        start_new_session=os.name != "nt",
    )
    requests = [
        {"jsonrpc": "2.0", "id": "tools", "method": "tools/list"},
        {
            "jsonrpc": "2.0",
            "id": "status",
            "method": "resources/read",
            "params": {"uri": STATUS_URI},
        },
    ]
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
                fail("serve_stdio", artifact, f"serve --stdio timed out after {timeout_secs}s")
            if line is None:
                terminate_process_tree(process)
                process_terminated = True
                stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
                write_stdio_artifact(artifact, transcript, "".join(stdout_lines), stderr_path)
                fail("serve_stdio", artifact, "serve --stdio closed before responding")
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
                fail("serve_stdio", artifact, f"serve --stdio emitted invalid JSON: {exc}")
            entry["response"] = response
            responses.append(response)
    finally:
        try:
            process.stdin.close()
        except OSError:
            pass
        if not process_terminated:
            try:
                process.wait(timeout=0.1)
            except subprocess.TimeoutExpired:
                terminate_process_tree(process)
                try:
                    process.wait(timeout=0.5)
                except subprocess.TimeoutExpired:
                    process.kill()
        stdout_thread.join(timeout=0.2)
        stderr_thread.join(timeout=0.2)

    stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
    responses_by_id = {response.get("id"): response for response in responses if isinstance(response, dict)}
    tools = responses_by_id.get("tools")
    status_response = responses_by_id.get("status")
    payload = {"tools": tools, "status_response": status_response, "transcript": transcript, "stdout": "".join(stdout_lines)}
    write_json(artifact, payload)
    require(isinstance(tools, dict), "serve_stdio", artifact, "serve --stdio did not return tools/list response")
    require(isinstance(status_response, dict), "serve_stdio", artifact, "serve --stdio did not return status response")
    if "error" in tools:
        fail("serve_stdio", artifact, f"tools/list failed: {tools['error']}")
    if "error" in status_response:
        fail("serve_stdio", artifact, f"status resource failed: {status_response['error']}")

    contents = status_response.get("result", {}).get("contents", [])
    content = next((item for item in contents if item.get("uri") == STATUS_URI), None)
    require(content is not None, "serve_stdio", artifact, "status response missing codestory://status")
    status = json.loads(content.get("text", "{}"))
    payload["status"] = status
    write_json(artifact, payload)
    return status


def require_stdio_ready(status: dict, artifact: Path) -> None:
    require(status.get("server_version") is not None, "serve_stdio", artifact, "status missing server_version")
    surfaces = status.get("allowed_surfaces", {})
    for name in ["packet", "search", "context"]:
        allowed = surfaces.get(name, {}).get("allowed")
        require(allowed is True, "serve_stdio", artifact, f"allowed_surfaces.{name}.allowed is {allowed!r}")


def run_gate(args: argparse.Namespace) -> None:
    archive = Path(args.archive).resolve()
    project = Path(args.project).resolve()
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="codestory-packaged-agent-proof-", dir=out_dir) as temp:
        unpacked = Path(temp) / "unpacked"
        unpacked.mkdir()
        unpack_archive(archive, unpacked)
        cli = find_cli(unpacked)

        summary = {
            "archive": str(archive),
            "cli": str(cli),
            "project": str(project),
            "artifacts": {},
        }
        write_json(out_dir / "summary.json", summary)

        version_artifact = out_dir / "version.txt"
        run_command(cli, "version", ["--version"], version_artifact, args.timeout_secs, parse_json=False)
        summary["artifacts"]["version"] = str(version_artifact)
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
                "tiny",
                "--format",
                "json",
                "--output-file",
                str(packet_artifact),
            ],
            packet_artifact,
            args.timeout_secs,
        )
        require_packet_ready(packet, packet_artifact)
        summary["artifacts"]["packet"] = str(packet_artifact)
        write_json(out_dir / "summary.json", summary)

        stdio_artifact = out_dir / "serve-stdio-status.json"
        status = stdio_status(cli, project, stdio_artifact, args.timeout_secs)
        require_stdio_ready(status, stdio_artifact)
        summary["artifacts"]["serve_stdio"] = str(stdio_artifact)
        write_json(out_dir / "summary.json", summary)

    print(f"packaged agent proof passed; artifacts={out_dir}")


def write_fake_cli(path: Path) -> None:
    fake = path / "fake_cli.py"
    fake.write_text(
        textwrap.dedent(
            r'''
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
            layer = sys.argv[1]
            if layer == "retrieval" and len(sys.argv) > 2:
                layer = "retrieval_status"
            if fail == f"{layer}_stderr":
                print("forced stderr failure", file=sys.stderr)
                raise SystemExit(3)
            if fail == layer:
                print("forced failure")
                raise SystemExit(2)
            if layer == "ready":
                emit({"verdicts": [{"goal": "agent_packet_search", "status": "ready"}]})
            elif layer == "doctor":
                emit({"retrieval_mode": "full"})
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
            elif layer == "serve":
                for line in sys.stdin:
                    request = json.loads(line)
                    if fail == "serve_timeout":
                        time.sleep(60)
                        continue
                    if request.get("method") == "tools/list":
                        result = {"tools": [{"name": "packet"}, {"name": "search"}, {"name": "context"}]}
                    else:
                        status = {
                            "server_version": "9.9.9",
                            "allowed_surfaces": {
                                "packet": {"allowed": True},
                                "search": {"allowed": True},
                                "context": {"allowed": True},
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


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-self-test-") as temp:
        root = Path(temp)
        stage = root / "pkg" / "codestory-cli-v9.9.9-test"
        stage.mkdir(parents=True)
        write_fake_cli(stage)
        archive = root / "fake.zip"
        with zipfile.ZipFile(archive, "w") as handle:
            for path in stage.rglob("*"):
                handle.write(path, path.relative_to(stage.parent).as_posix())
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
            timeout_secs=30,
        )
        run_gate(args)
        assert (out_dir / "summary.json").is_file()

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
        timeout_args.timeout_secs = 1
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

    print("self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Gate releases on packaged full-sidecar agent proof.")
    parser.add_argument("--archive", help="Packaged codestory-cli archive to test.")
    parser.add_argument("--project", default=".", help="Representative repository to prove against.")
    parser.add_argument("--out-dir", default="target/packaged-agent-proof", help="Artifact directory.")
    parser.add_argument("--query", default=DEFAULT_QUERY, help="Search proof query.")
    parser.add_argument("--context-query", default=DEFAULT_QUERY, help="Context proof target query.")
    parser.add_argument("--question", default=DEFAULT_QUESTION, help="Packet proof question.")
    parser.add_argument("--timeout-secs", type=int, default=1800, help="Per-layer timeout.")
    parser.add_argument("--self-test", action="store_true", help="Run script self-tests.")
    args = parser.parse_args()
    if not args.self_test and not args.archive:
        parser.error("--archive is required unless --self-test is set")
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
