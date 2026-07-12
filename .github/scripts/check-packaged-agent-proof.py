#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import ctypes
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


def remove_tree_with_retry(path: Path, timeout_secs: float = 10.0, platform: str = os.name) -> None:
    deadline = time.monotonic() + timeout_secs
    while True:
        try:
            shutil.rmtree(path)
            return
        except FileNotFoundError:
            return
        except PermissionError as exc:
            if platform != "nt" or getattr(exc, "winerror", None) not in {5, 32}:
                raise
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.2)


@contextlib.contextmanager
def temporary_directory_with_retry(prefix: str, directory: Path):
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=directory))
    try:
        yield str(path)
    finally:
        remove_tree_with_retry(path)


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


def proof_environment(base: dict[str, str]) -> dict[str, str]:
    env = dict(base)
    if hasattr(os, "getuid") and hasattr(os, "getgid"):
        env["CODESTORY_QDRANT_USER"] = f"{os.getuid()}:{os.getgid()}"
        env["CODESTORY_QDRANT_SNAPSHOTS_PATH"] = "/qdrant/storage/snapshots"
    return env


def validated_proof_compose_state(cache_root: Path, project: Path, read_state) -> tuple[Path, dict] | None:
    if cache_root.is_symlink() or project.is_symlink():
        raise RuntimeError("proof cache and project roots must not be symlinks")
    cache_root = cache_root.resolve(strict=True)
    project = project.resolve(strict=True)
    state_file = cache_root / "retrieval-sidecars.json"
    if state_file.is_symlink():
        raise RuntimeError(f"proof sidecar state must not be a symlink: {state_file}")
    if not state_file.exists():
        return None
    if not state_file.is_file():
        raise RuntimeError(f"proof sidecar state is not a regular file: {state_file}")
    state = read_state(state_file)
    if not isinstance(state, dict):
        raise TypeError(f"proof sidecar state is not an object: {state_file}")
    expected_identity = {
        "owner": "codestory",
        "profile": "local",
        "namespace": "codestory",
        "compose_project": "codestory",
    }
    for name, expected in expected_identity.items():
        if state.get(name) != expected:
            raise RuntimeError(f"proof sidecar state {name} does not match {expected!r}: {state_file}")
    state = dict(state)
    for name, expected in (("qdrant_data_dir", cache_root / "qdrant"),):
        value = state.get(name)
        if not isinstance(value, str) or not value:
            raise TypeError(f"proof sidecar state {name} is not a path string: {state_file}")
        observed = Path(value)
        expected = expected.resolve(strict=True)
        if observed != expected or observed.is_symlink() or observed.resolve(strict=True) != expected:
            raise RuntimeError(f"proof sidecar state {name} escaped its cache root: {state_file}")
    lexical_expected = cache_root / "lexical"
    lexical_value = state.get("lexical_data_dir")
    if lexical_value is None:
        lexical_value = state.get("zoekt_data_dir")
    if not isinstance(lexical_value, str) or not lexical_value:
        raise TypeError(f"proof sidecar state lexical_data_dir or legacy zoekt_data_dir is not a path string: {state_file}")
    observed = Path(lexical_value)
    if observed != lexical_expected or observed.is_symlink():
        raise RuntimeError(f"proof sidecar state lexical_data_dir escaped its cache root: {state_file}")
    if observed.exists() and observed.resolve(strict=True) != lexical_expected:
        raise RuntimeError(f"proof sidecar state lexical_data_dir escaped its cache root: {state_file}")
    state["lexical_data_dir"] = str(lexical_expected)
    compose_value = state.get("compose_file")
    if not isinstance(compose_value, str) or not compose_value:
        raise TypeError(f"proof sidecar compose_file is not a path string: {state_file}")
    compose_file = Path(compose_value)
    if compose_file.is_symlink() or not compose_file.is_file():
        raise RuntimeError(f"proof sidecar compose file is not a regular canonical file: {compose_file}")
    allowed_compose_files = set()
    for candidate in (
        project / "docker" / "retrieval-compose.yml",
        cache_root / "retrieval-compose.yml",
    ):
        if not candidate.is_file() or candidate.is_symlink():
            continue
        canonical_candidate = candidate.resolve(strict=True)
        if candidate == canonical_candidate:
            allowed_compose_files.add(canonical_candidate)
    canonical_compose_file = compose_file.resolve(strict=True)
    if compose_file != canonical_compose_file or canonical_compose_file not in allowed_compose_files:
        raise RuntimeError(f"proof sidecar compose file is outside the allowed roots: {compose_file}")
    return state_file, state


def cleanup_proof_compose(
    cache_root: Path,
    project: Path,
    env: dict[str, str],
    results: list[dict],
    run,
    read_state,
) -> None:
    try:
        validated = validated_proof_compose_state(cache_root, project, read_state)
    except Exception as exc:
        results.append(
            {
                "kind": "compose_state_validation",
                "state_file": str(cache_root / "retrieval-sidecars.json"),
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        raise RuntimeError(f"proof-owned Compose state validation failed: {exc}") from exc
    if validated is None:
        return
    state_file, state = validated
    compose_env = {
        **env,
        "CODESTORY_SIDECAR_NAMESPACE": state["namespace"],
        "CODESTORY_QDRANT_DATA_DIR": state["qdrant_data_dir"],
        "CODESTORY_EMBED_MODEL_DIR": state["qdrant_data_dir"],
    }
    command = [
        "docker",
        "compose",
        "-p",
        state["compose_project"],
        "-f",
        state["compose_file"],
        "down",
        "--remove-orphans",
    ]
    try:
        result = run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30,
            check=False,
            env=compose_env,
            text=True,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        results.append({"kind": "compose_down", "state_file": str(state_file), "error": str(exc)})
        raise RuntimeError(f"could not stop proof-owned Compose sidecars: {exc}") from exc
    results.append(
        {
            "kind": "compose_down",
            "state_file": str(state_file),
            "returncode": result.returncode,
            "stdout": result.stdout,
            "stderr": result.stderr,
        }
    )
    if result.returncode != 0:
        raise RuntimeError(f"proof-owned Compose cleanup exited {result.returncode}")


def capture_proof_compose_diagnostics(
    cache_root: Path,
    project: Path,
    env: dict[str, str],
    artifact: Path,
    run=subprocess.run,
    read_state=read_json_file,
) -> None:
    payload = {"state_file": str(cache_root / "retrieval-sidecars.json"), "commands": []}
    try:
        validated = validated_proof_compose_state(cache_root, project, read_state)
        if validated is None:
            payload["error"] = "proof-owned Compose state is absent"
            write_json(artifact, payload)
            return
        _, state = validated
        compose_env = {
            **env,
            "CODESTORY_SIDECAR_NAMESPACE": state["namespace"],
            "CODESTORY_QDRANT_DATA_DIR": state["qdrant_data_dir"],
            "CODESTORY_EMBED_MODEL_DIR": state["qdrant_data_dir"],
        }
        base = ["docker", "compose", "-p", state["compose_project"], "-f", state["compose_file"]]
        for kind, extra in (
            ("compose_ps", ["ps", "--all"]),
            ("qdrant_logs", ["logs", "--no-color", "--tail", "200", "qdrant"]),
        ):
            try:
                result = run(
                    [*base, *extra],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    timeout=20,
                    check=False,
                    env=compose_env,
                    text=True,
                )
                payload["commands"].append(
                    {
                        "kind": kind,
                        "returncode": result.returncode,
                        "stdout": result.stdout,
                        "stderr": result.stderr,
                    }
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                payload["commands"].append({"kind": kind, "error": f"{type(exc).__name__}: {exc}"})
    except Exception as exc:
        payload["error"] = f"{type(exc).__name__}: {exc}"
    write_json(artifact, payload)


def cleanup_proof_cache(
    cli: Path,
    project: Path,
    cache_root: Path,
    artifact: Path,
    run=subprocess.run,
    read_state=read_json_file,
) -> None:
    env = {**os.environ, "CODESTORY_CACHE_ROOT": str(cache_root)}
    results = []
    try:
        cleanup_proof_compose(cache_root, project, env, results, run, read_state)
    except Exception as exc:
        write_json(
            artifact,
            {"cache_root": str(cache_root), "commands": results, "removed": False, "error": str(exc)},
        )
        raise
    for profile, extra in (("local", []), ("agent", ["--run-id", "shared-agent"])):
        try:
            result = run(
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
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=20,
                check=False,
                env=env,
                text=True,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            results.append({"profile": profile, "error": str(exc)})
            write_json(artifact, {"cache_root": str(cache_root), "commands": results, "removed": False})
            raise RuntimeError(f"could not stop {profile} proof sidecars: {exc}") from exc
        results.append(
            {
                "kind": "retrieval_down",
                "profile": profile,
                "returncode": result.returncode,
                "stdout": result.stdout,
                "stderr": result.stderr,
            }
        )
        if result.returncode != 0:
            write_json(artifact, {"cache_root": str(cache_root), "commands": results, "removed": False})
            raise RuntimeError(f"{profile} proof sidecar cleanup exited {result.returncode}")
    try:
        remove_tree_with_retry(cache_root)
    except OSError as exc:
        write_json(
            artifact,
            {"cache_root": str(cache_root), "commands": results, "removed": False, "error": str(exc)},
        )
        raise
    removed = not cache_root.exists()
    write_json(artifact, {"cache_root": str(cache_root), "commands": results, "removed": removed})
    if not removed:
        raise RuntimeError(f"proof cache still exists after cleanup: {cache_root}")


def cleanup_proof_cache_on_exit(
    cli: Path,
    project: Path,
    cache_root: Path,
    artifact: Path,
    run=subprocess.run,
    read_state=read_json_file,
):
    def cleanup(exc_type, exc, traceback) -> bool:
        try:
            cleanup_proof_cache(cli, project, cache_root, artifact, run, read_state)
        except Exception as cleanup_exc:
            if exc is None:
                raise
            if hasattr(exc, "add_note"):
                exc.add_note(f"proof cleanup also failed: {type(cleanup_exc).__name__}: {cleanup_exc}")
        return False

    return cleanup


def local_profile_environment(base: dict[str, str]) -> dict[str, str]:
    """Keep explicit local commands and their default runtime in one namespace."""
    return {**base, "CODESTORY_SIDECAR_PROFILE": "local"}


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
    manifest = payload.get("manifest")
    lexical_version = manifest.get("lexical_version") if isinstance(manifest, dict) else None
    require(
        lexical_version == "sqlite-fts5-v1",
        layer,
        artifact,
        f"manifest.lexical_version is {lexical_version!r}, expected 'sqlite-fts5-v1'",
    )
    for field in ["qdrant_stubbed", "scip_stubbed"]:
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


def windows_process_api():
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    kernel32.OpenProcess.restype = ctypes.c_void_p
    kernel32.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    kernel32.WaitForSingleObject.restype = ctypes.c_ulong
    kernel32.TerminateProcess.argtypes = [ctypes.c_void_p, ctypes.c_uint]
    kernel32.TerminateProcess.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    return kernel32


def windows_last_error(kernel32) -> int:
    return int(getattr(kernel32, "last_error", ctypes.get_last_error()))


def require_windows_process_exit(process_handle, pid: int, timeout_ms: int = 10_000, kernel32=None) -> None:
    kernel32 = kernel32 or windows_process_api()
    wait_result = kernel32.WaitForSingleObject(process_handle, timeout_ms)
    if wait_result == 0:
        return
    if wait_result == 0xFFFFFFFF:
        raise RuntimeError(f"could not wait for worker process {pid} (Windows error {windows_last_error(kernel32)})")
    raise RuntimeError(f"worker process {pid} did not terminate before cleanup (wait result {wait_result})")


def terminate_worker_pid(
    pid: int,
    diagnostics: dict | None = None,
    *,
    kernel32=None,
    run=subprocess.run,
    platform: str | None = None,
) -> None:
    diagnostics = diagnostics if diagnostics is not None else {}
    platform = platform or os.name
    diagnostics.update({"pid": pid, "platform": platform, "attempts": []})
    if pid <= 0:
        diagnostics["status"] = "invalid_pid"
        return
    if platform == "nt":
        kernel32 = kernel32 or windows_process_api()
        process_handle = None
        open_error = 0
        try:
            process_handle = kernel32.OpenProcess(0x00100001, False, pid)
            if not process_handle:
                open_error = windows_last_error(kernel32)
        except (AttributeError, OSError):
            process_handle = None
        diagnostics["attempts"].append(
            {"kind": "open_process", "success": bool(process_handle), "windows_error": open_error}
        )
        if not process_handle:
            if open_error == 87:
                diagnostics["status"] = "already_exited"
                return
            raise RuntimeError(
                f"could not open worker process {pid} for termination proof (Windows error {open_error})"
            )
        taskkill_evidence = "not run"
        try:
            taskkill = run(
                ["taskkill", "/PID", str(pid), "/T", "/F"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=10,
                check=False,
                text=True,
            )
            taskkill_evidence = (
                f"exit={taskkill.returncode} stdout={taskkill.stdout.strip()!r} stderr={taskkill.stderr.strip()!r}"
            )
            diagnostics["attempts"].append(
                {
                    "kind": "taskkill",
                    "returncode": taskkill.returncode,
                    "stdout": taskkill.stdout,
                    "stderr": taskkill.stderr,
                }
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            taskkill_evidence = f"{type(exc).__name__}: {exc}"
            diagnostics["attempts"].append({"kind": "taskkill", "error": taskkill_evidence})
        try:
            try:
                require_windows_process_exit(process_handle, pid, kernel32=kernel32)
                diagnostics["status"] = "terminated_after_taskkill"
            except RuntimeError as wait_error:
                diagnostics["attempts"].append({"kind": "initial_wait", "error": str(wait_error)})
                terminated = bool(kernel32.TerminateProcess(process_handle, 1))
                terminate_error = 0 if terminated else windows_last_error(kernel32)
                diagnostics["attempts"].append(
                    {"kind": "terminate_process", "success": terminated, "windows_error": terminate_error}
                )
                if not terminated:
                    try:
                        require_windows_process_exit(process_handle, pid, timeout_ms=0, kernel32=kernel32)
                    except RuntimeError:
                        raise RuntimeError(
                            f"could not terminate worker process {pid} (Windows error {terminate_error}; "
                            f"taskkill {taskkill_evidence}; initial wait: {wait_error})"
                        ) from wait_error
                try:
                    require_windows_process_exit(process_handle, pid, timeout_ms=30_000, kernel32=kernel32)
                    diagnostics["status"] = "terminated_after_direct_termination"
                except RuntimeError as final_wait_error:
                    diagnostics["attempts"].append({"kind": "final_wait", "error": str(final_wait_error)})
                    raise RuntimeError(
                        f"worker process {pid} remained alive after direct termination; "
                        f"taskkill {taskkill_evidence}; initial wait: {wait_error}; final wait: {final_wait_error}"
                    ) from final_wait_error
        finally:
            kernel32.CloseHandle(process_handle)
    else:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            diagnostics["status"] = "already_exited"
            return
        except PermissionError as exc:
            raise RuntimeError(f"could not terminate worker process {pid}") from exc
        deadline = time.monotonic() + 10
        while True:
            try:
                exited = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                if exited is not None:
                    diagnostics["status"] = "terminated"
                    return
            except ChildProcessError:
                pass
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                diagnostics["status"] = "terminated"
                return
            except PermissionError as exc:
                raise RuntimeError(f"could not prove worker process {pid} terminated") from exc
            if time.monotonic() >= deadline:
                raise RuntimeError(f"worker process {pid} did not terminate before cleanup")
            time.sleep(0.1)


def proof_started_repair_workers(responses: list[dict]) -> list[dict]:
    responses_by_id = {response.get("id"): response for response in responses if isinstance(response, dict)}
    repair = responses_by_id.get("repair", {}).get("result", {}).get("structuredContent", {})
    if not isinstance(repair, dict) or repair.get("status") != "started":
        return []
    pid = repair.get("pid")
    attempt_id = repair.get("attempt_id")
    if not isinstance(pid, int) or not isinstance(attempt_id, str) or not attempt_id:
        return []
    status = status_from_resource_response(responses_by_id.get("status_after_repair", {}))
    if not isinstance(status, dict):
        return []
    setup = status.get("sidecar_setup", {})
    observed_attempts = {
        (setup.get("active_repair") or {}).get("attempt_id"),
        (setup.get("last_worker_result") or {}).get("attempt_id"),
    }
    if attempt_id not in observed_attempts:
        return []
    return [{"pid": pid, "attempt_id": attempt_id, "source": "proof_started_repair_response"}]


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
    with temporary_directory_with_retry("codestory-plugin-data-", artifact.parent) as data:
        return stdio_status_command(
            ["node", str(launcher)],
            artifact,
            timeout_secs,
            project,
            layer="plugin_stdio",
            cwd=project,
            env=proof_environment({
                **os.environ,
                "CODESTORY_CLI": "",
                "CODESTORY_CACHE_ROOT": str(cache_root),
                "CODESTORY_PLUGIN_RELEASE_DIR": str(release_dir),
                "PLUGIN_DATA": data,
            }),
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
    with temporary_directory_with_retry("codestory-plugin-data-", artifact.parent) as data:
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
            env=proof_environment({
                **os.environ,
                "CODESTORY_CLI": "",
                "CODESTORY_CACHE_ROOT": str(cache_root),
                "CODESTORY_PLUGIN_RELEASE_DIR": str(release_dir),
                "CODESTORY_PLUGIN_SIDECAR_POLICY_PATH": str(policy_path),
                "PLUGIN_DATA": str(plugin_data),
            }),
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
    failure_pending = False
    failure_extra: dict | None = None

    def stdio_fail(message: str, extra: dict | None = None) -> None:
        nonlocal failure_pending, failure_extra
        failure_pending = True
        failure_extra = extra
        fail(layer, artifact, message)

    def read_correlated_response(expected_id: str | int, entry: dict, phase: str) -> dict:
        deadline = time.monotonic() + timeout_secs
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                entry["timed_out"] = True
                stdio_fail(f"stdio MCP {phase} timed out after {timeout_secs}s", {"timed_out": True})
            try:
                line = read_stdio_line(stdout_queue, remaining)
            except subprocess.TimeoutExpired:
                entry["timed_out"] = True
                stdio_fail(f"stdio MCP {phase} timed out after {timeout_secs}s", {"timed_out": True})
            if line is None:
                stdio_fail(f"stdio MCP server closed during {phase}")
            try:
                response = json.loads(line)
            except json.JSONDecodeError as exc:
                entry["invalid_line"] = line
                stdio_fail(f"stdio MCP {phase} emitted invalid JSON: {exc}", {"invalid_line": line})
            if not isinstance(response, dict):
                entry["invalid_response"] = response
                stdio_fail(f"stdio MCP {phase} emitted a non-object response", {"invalid_response": response})
            if response.get("jsonrpc") != "2.0":
                entry["invalid_response"] = response
                stdio_fail(f"stdio MCP {phase} response has invalid jsonrpc envelope", {"invalid_response": response})
            if "method" in response:
                method = response.get("method")
                if isinstance(method, str) and method.startswith("notifications/") and "id" not in response:
                    transcript.append({"notification": response})
                    continue
                entry["unexpected_server_message"] = response
                stdio_fail(f"stdio MCP {phase} emitted an unexpected server request", {"unexpected_server_message": response})
            if response.get("id") != expected_id:
                entry["unexpected_response"] = response
                stdio_fail(
                    f"stdio MCP {phase} returned id {response.get('id')!r}, expected {expected_id!r}",
                    {"unexpected_response": response},
                )
            if ("result" in response) == ("error" in response):
                entry["invalid_response"] = response
                stdio_fail(f"stdio MCP {phase} response must contain exactly one of result or error", {"invalid_response": response})
            return response

    try:
        initialize = {
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "packaged-proof", "version": "1"}},
        }
        initialize_entry = {"request": initialize}
        transcript.append(initialize_entry)
        process.stdin.write(json.dumps(initialize) + "\n")
        process.stdin.flush()
        initialize_response = read_correlated_response("initialize", initialize_entry, "initialize")
        initialize_entry["response"] = initialize_response
        responses.append(initialize_response)
        initialize_result = initialize_response.get("result")
        if "error" in initialize_response:
            stdio_fail(f"stdio MCP initialize failed: {initialize_response.get('error')}")
        if not isinstance(initialize_result, dict):
            stdio_fail("stdio MCP initialize missing result")
        if initialize_result.get("protocolVersion") != initialize["params"]["protocolVersion"]:
            stdio_fail(f"stdio MCP initialize protocol is {initialize_result.get('protocolVersion')!r}")
        if not isinstance(initialize_result.get("capabilities"), dict):
            stdio_fail("stdio MCP initialize missing capabilities")
        if not isinstance(initialize_result["capabilities"].get("tools"), dict):
            stdio_fail("stdio MCP initialize capabilities.tools must be an object")
        initialized = {"jsonrpc": "2.0", "method": "notifications/initialized"}
        transcript.append({"request": initialized})
        process.stdin.write(json.dumps(initialized) + "\n")
        process.stdin.flush()

        list_changed = initialize_result.get("capabilities", {}).get("tools", {}).get("listChanged")
        if list_changed is True:
            publication_deadline = time.monotonic() + timeout_secs
            while True:
                remaining = publication_deadline - time.monotonic()
                if remaining <= 0:
                    stdio_fail(f"stdio MCP runtime publication timed out after {timeout_secs}s", {"timed_out": True})
                try:
                    notification_line = read_stdio_line(stdout_queue, remaining)
                except subprocess.TimeoutExpired:
                    stdio_fail(f"stdio MCP runtime publication timed out after {timeout_secs}s", {"timed_out": True})
                if notification_line is None:
                    stdio_fail("stdio MCP server closed before runtime publication")
                try:
                    notification = json.loads(notification_line)
                except json.JSONDecodeError as exc:
                    stdio_fail(f"stdio MCP runtime publication emitted invalid JSON: {exc}", {"invalid_line": notification_line})
                if not isinstance(notification, dict):
                    stdio_fail("stdio MCP runtime publication emitted a non-object message", {"invalid_response": notification})
                method = notification.get("method")
                if (
                    notification.get("jsonrpc") != "2.0"
                    or "id" in notification
                    or not isinstance(method, str)
                    or not method.startswith("notifications/")
                ):
                    stdio_fail("stdio MCP runtime publication emitted an unexpected message", {"unexpected_response": notification})
                transcript.append({"notification": notification})
                if method == "notifications/tools/list_changed":
                    break

        for request in requests:
            entry = {"request": request}
            transcript.append(entry)
            process.stdin.write(json.dumps(request) + "\n")
            process.stdin.flush()
            response = read_correlated_response(request["id"], entry, f"request {request['id']}")
            entry["response"] = response
            responses.append(response)
    finally:
        try:
            process.stdin.close()
        except OSError:
            pass
        if not process_terminated:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                terminate_process_tree(process)
            process_terminated = True
        worker_cleanup_artifact = artifact.with_name(f"{artifact.stem}-worker-cleanup.json")
        worker_cleanup = []
        owned_workers = proof_started_repair_workers(responses) if cleanup_status_workers else []
        for worker in owned_workers:
            evidence = dict(worker)
            worker_cleanup.append(evidence)
            try:
                terminate_worker_pid(worker["pid"], evidence)
            except Exception as exc:
                evidence["error"] = f"{type(exc).__name__}: {exc}"
                write_json(worker_cleanup_artifact, {"workers": worker_cleanup})
                raise
            write_json(worker_cleanup_artifact, {"workers": worker_cleanup})
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        stdout_thread.join(timeout=2)
        stderr_thread.join(timeout=2)
        if failure_pending:
            stderr_path.write_text("".join(stderr_lines), encoding="utf-8")
            write_stdio_artifact(artifact, transcript, "".join(stdout_lines), stderr_path, failure_extra)

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
        proof_cleanup_artifact = out_dir / "proof-cache-cleanup.json"
        cleanup_stack.push(cleanup_proof_cache_on_exit(cli, project, cache_root, proof_cleanup_artifact))
        proof_env = proof_environment({**os.environ, "CODESTORY_CACHE_ROOT": str(cache_root)})
        stdio_cache_root = Path(tempfile.mkdtemp(prefix="codestory-packaged-stdio-cache-"))
        stdio_cleanup_artifact = out_dir / "stdio-cache-cleanup.json"
        cleanup_stack.push(cleanup_proof_cache_on_exit(cli, project, stdio_cache_root, stdio_cleanup_artifact))
        stdio_env = {**os.environ, "CODESTORY_CACHE_ROOT": str(stdio_cache_root)}

        summary = {
            "archive": str(archive),
            "cli": str(cli),
            "plugin_skill": str(plugin_skill),
            "project": str(project),
            "artifacts": {
                "proof_cache_cleanup": str(proof_cleanup_artifact),
                "stdio_cache_cleanup": str(stdio_cleanup_artifact),
            },
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

        local_env = local_profile_environment(proof_env)
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
            env=local_env,
        )
        summary["artifacts"]["local_retrieval_bootstrap"] = str(local_bootstrap_artifact)
        write_json(out_dir / "summary.json", summary)

        local_index_artifact = out_dir / "local-retrieval-index.json"
        try:
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
                env=local_env,
            )
            require_retrieval_index_ready(local_index, "local_retrieval_index", local_index_artifact)
        except GateFailure:
            compose_diagnostics_artifact = out_dir / "local-retrieval-compose-diagnostics.json"
            capture_proof_compose_diagnostics(cache_root, project, local_env, compose_diagnostics_artifact)
            summary["artifacts"]["local_retrieval_compose_diagnostics"] = str(compose_diagnostics_artifact)
            write_json(out_dir / "summary.json", summary)
            raise
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
            env=local_env,
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

        stdio_status_payload = stdio_status(cli, project, stdio_artifact, args.timeout_secs, proof_env)
        require_stdio_shape(stdio_status_payload, stdio_artifact, args.expected_version)
        allowed = stdio_status_payload.get("allowed_surfaces", {})
        if not all(allowed.get(name, {}).get("allowed") is True for name in ("packet", "search", "context")):
            shutil.copy2(stdio_artifact, out_dir / "serve-stdio-status-initial.json")
            stdio_status_payload = stdio_status(cli, project, stdio_artifact, args.timeout_secs, proof_env)
        require_stdio_ready(stdio_status_payload, stdio_artifact, args.expected_version)
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
                emit({
                    "manifest": {"lexical_version": "sqlite-fts5-v1"},
                    "qdrant_stubbed": False,
                    "scip_stubbed": False,
                })
            elif layer == "retrieval_status":
                emit({"retrieval_mode": "full"})
            elif layer == "retrieval_down":
                emit({"stopped": True})
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
                    if request.get("method") == "notifications/initialized":
                        if os.environ.get("CODESTORY_FAKE_LIST_CHANGED") == "1":
                            for method in (
                                "notifications/tools/list_changed",
                                "notifications/resources/list_changed",
                                "notifications/prompts/list_changed",
                            ):
                                print(json.dumps({"jsonrpc": "2.0", "method": method}), flush=True)
                        continue
                    if request.get("method") == "initialize":
                        result = {
                            "protocolVersion": request.get("params", {}).get("protocolVersion", "2024-11-05"),
                            "capabilities": {
                                "tools": {"listChanged": os.environ.get("CODESTORY_FAKE_LIST_CHANGED") == "1"},
                                "resources": {"listChanged": False},
                            },
                            "serverInfo": {"name": "codestory", "version": "9.9.9"},
                        }
                    elif request.get("method") == "tools/list":
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
                    response_id = request.get("id")
                    initialize_mode = os.environ.get("CODESTORY_FAKE_INITIALIZE_MODE")
                    if request.get("method") == "initialize" and initialize_mode == "non_object":
                        print(json.dumps([]), flush=True)
                        continue
                    if request.get("method") == "initialize" and initialize_mode == "wrong_id":
                        response_id = "wrong-initialize"
                    if request.get("method") == "initialize" and initialize_mode == "error":
                        print(json.dumps({"jsonrpc": "2.0", "id": response_id, "error": {"code": -32000, "message": "synthetic initialize error"}}), flush=True)
                        continue
                    if request.get("method") == "initialize" and initialize_mode == "malformed_tools":
                        result["capabilities"]["tools"] = []
                    if request.get("method") == "initialize" and initialize_mode == "malformed_jsonrpc":
                        print("synthetic protocol stderr", file=sys.stderr, flush=True)
                    if request.get("method") == "tools/list" and initialize_mode == "out_of_order":
                        response_id = "resources"
                    if request.get("method") == "tools/list" and initialize_mode == "server_request_collision":
                        print(json.dumps({"jsonrpc": "2.0", "id": response_id, "method": "sampling/createMessage", "params": {}}), flush=True)
                        continue
                    if request.get("method") == "tools/list" and initialize_mode == "malformed_method":
                        print(json.dumps({"jsonrpc": "2.0", "method": 42}), flush=True)
                        continue
                    jsonrpc = "1.0" if request.get("method") == "initialize" and initialize_mode == "malformed_jsonrpc" else "2.0"
                    print(json.dumps({"jsonrpc": jsonrpc, "id": response_id, "result": result}), flush=True)
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
    base_environment = {"KEEP": "value", "CODESTORY_SIDECAR_PROFILE": "agent"}
    owned_environment = proof_environment(base_environment)
    if hasattr(os, "getuid") and hasattr(os, "getgid"):
        assert owned_environment["CODESTORY_QDRANT_USER"] == f"{os.getuid()}:{os.getgid()}"
        assert owned_environment["CODESTORY_QDRANT_SNAPSHOTS_PATH"] == "/qdrant/storage/snapshots"
    local_environment = local_profile_environment(base_environment)
    assert local_environment["KEEP"] == "value"
    assert local_environment["CODESTORY_SIDECAR_PROFILE"] == "local"
    assert base_environment["CODESTORY_SIDECAR_PROFILE"] == "agent"

    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-self-test-") as temp:
        root = Path(temp)
        transient_cleanup_attempts = 0
        original_rmtree = shutil.rmtree

        class SimulatedWindowsLock(PermissionError):
            winerror = 5

        def transient_cleanup(path):
            nonlocal transient_cleanup_attempts
            transient_cleanup_attempts += 1
            if transient_cleanup_attempts == 1:
                raise SimulatedWindowsLock("simulated transient executable lock")
            original_rmtree(path)

        retry_root = root / "cleanup-retry"
        retry_root.mkdir()
        shutil.rmtree = transient_cleanup
        try:
            remove_tree_with_retry(retry_root, timeout_secs=1, platform="nt")
        finally:
            shutil.rmtree = original_rmtree
        assert transient_cleanup_attempts == 2

        persistent_root = root / "cleanup-persistent"
        persistent_root.mkdir()

        def persistent_cleanup(_path):
            raise SimulatedWindowsLock("simulated persistent lock")

        shutil.rmtree = persistent_cleanup
        try:
            try:
                remove_tree_with_retry(persistent_root, timeout_secs=0, platform="nt")
            except PermissionError:
                pass
            else:
                raise AssertionError("persistent cleanup lock should remain fail-closed")
        finally:
            shutil.rmtree = original_rmtree
            original_rmtree(persistent_root)

        unclassified_root = root / "cleanup-unclassified"
        unclassified_root.mkdir()
        unclassified_attempts = 0

        def unclassified_cleanup(_path):
            nonlocal unclassified_attempts
            unclassified_attempts += 1
            raise PermissionError("simulated ACL failure")

        shutil.rmtree = unclassified_cleanup
        try:
            try:
                remove_tree_with_retry(unclassified_root, timeout_secs=1, platform="nt")
            except PermissionError:
                pass
            else:
                raise AssertionError("unclassified cleanup failures must not be retried")
        finally:
            shutil.rmtree = original_rmtree
            original_rmtree(unclassified_root)
        assert unclassified_attempts == 1

        if os.name == "nt":
            kernel32 = windows_process_api()
            process_handle = kernel32.OpenProcess(0x00100000, False, os.getpid())
            assert process_handle
            try:
                try:
                    require_windows_process_exit(process_handle, os.getpid(), timeout_ms=0, kernel32=kernel32)
                except RuntimeError:
                    pass
                else:
                    raise AssertionError("an unterminated worker must remain a cleanup failure")
            finally:
                kernel32.CloseHandle(process_handle)

            class FakeKernel32:
                def __init__(self, waits, *, handle=123, terminate=True, last_error=5):
                    self.waits = list(waits)
                    self.handle = handle
                    self.terminate = terminate
                    self.last_error = last_error

                def OpenProcess(self, _access, _inherit, _pid):
                    return self.handle

                def WaitForSingleObject(self, _handle, _timeout):
                    return self.waits.pop(0)

                def TerminateProcess(self, _handle, _exit_code):
                    return self.terminate

                def CloseHandle(self, _handle):
                    return True

            failed_wait = FakeKernel32([0xFFFFFFFF], last_error=6)
            try:
                require_windows_process_exit(123, 42, timeout_ms=0, kernel32=failed_wait)
            except RuntimeError as exc:
                assert "Windows error 6" in str(exc)
            else:
                raise AssertionError("WAIT_FAILED must remain a cleanup failure")

            open_failure = FakeKernel32([], handle=None, last_error=5)
            open_diagnostics = {}
            try:
                terminate_worker_pid(
                    42,
                    open_diagnostics,
                    kernel32=open_failure,
                    run=lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 128, "", "not found"),
                    platform="nt",
                )
            except RuntimeError as exc:
                assert "Windows error 5" in str(exc)
            else:
                raise AssertionError("OpenProcess access failure must remain fail closed")
            assert open_diagnostics["attempts"] == [
                {"kind": "open_process", "success": False, "windows_error": 5}
            ]

            async_exit = FakeKernel32([258, 0])
            async_diagnostics = {}

            def timed_out_taskkill(*_args, **_kwargs):
                raise subprocess.TimeoutExpired("taskkill", 10)

            terminate_worker_pid(
                42,
                async_diagnostics,
                kernel32=async_exit,
                run=timed_out_taskkill,
                platform="nt",
            )
            assert async_diagnostics["status"] == "terminated_after_direct_termination"
            assert "TimeoutExpired" in async_diagnostics["attempts"][1]["error"]

            terminate_failure = FakeKernel32([258, 258], terminate=False, last_error=5)
            terminate_diagnostics = {}
            try:
                terminate_worker_pid(
                    42,
                    terminate_diagnostics,
                    kernel32=terminate_failure,
                    run=lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 1, "", "denied"),
                    platform="nt",
                )
            except RuntimeError as exc:
                assert "Windows error 5" in str(exc)
            else:
                raise AssertionError("TerminateProcess failure must remain fail closed")
            assert terminate_diagnostics["attempts"][1]["returncode"] == 1
            assert terminate_diagnostics["attempts"][-1]["success"] is False

        stage = root / "pkg" / "codestory-cli-v9.9.9-test"
        stage.mkdir(parents=True)
        write_fake_cli(stage)
        fake_cli = find_cli(stage)
        compose_file = root / "docker" / "retrieval-compose.yml"
        compose_file.parent.mkdir()
        compose_file.write_text("services: {}\n", encoding="utf-8")

        def write_proof_compose_state(cache_root: Path, overrides: dict | None = None) -> Path:
            cache_root.mkdir()
            (cache_root / "qdrant").mkdir()
            (cache_root / "lexical").mkdir()
            state = {
                "owner": "codestory",
                "profile": "local",
                "namespace": "codestory",
                "compose_project": "codestory",
                "compose_file": str(compose_file),
                "qdrant_data_dir": str(cache_root / "qdrant"),
                "lexical_data_dir": str(cache_root / "lexical"),
            }
            state.update(overrides or {})
            state_file = cache_root / "retrieval-sidecars.json"
            write_json(state_file, state)
            return state_file

        compose_cleanup_root = root / "compose-cleanup"
        write_proof_compose_state(compose_cleanup_root)
        compose_cleanup_artifact = root / "compose-cleanup.json"
        compose_calls = []

        def successful_compose_cleanup(command, **kwargs):
            if command[0] == "docker":
                compose_calls.append((command, kwargs["env"]))
                return subprocess.CompletedProcess(command, 0, "stopped", "")
            return subprocess.run(command, **kwargs)

        cleanup_proof_cache(
            fake_cli,
            root,
            compose_cleanup_root,
            compose_cleanup_artifact,
            successful_compose_cleanup,
        )
        assert not compose_cleanup_root.exists()
        assert compose_calls[0][0][-2:] == ["down", "--remove-orphans"]
        assert compose_calls[0][1]["CODESTORY_SIDECAR_NAMESPACE"] == "codestory"
        compose_cleanup = read_json_file(compose_cleanup_artifact)
        assert compose_cleanup["removed"] is True
        assert compose_cleanup["commands"][0]["kind"] == "compose_down"

        optional_lexical_root = root / "compose-cleanup-optional-lexical"
        write_proof_compose_state(optional_lexical_root, {"lexical_data_dir": None})
        optional_lexical_artifact = root / "compose-cleanup-optional-lexical.json"
        try:
            cleanup_proof_cache(fake_cli, root, optional_lexical_root, optional_lexical_artifact)
        except RuntimeError:
            pass
        else:
            raise AssertionError("missing canonical and legacy lexical ownership must fail closed")
        assert read_json_file(optional_lexical_artifact)["commands"][0]["kind"] == "compose_state_validation"
        remove_tree_with_retry(optional_lexical_root)

        legacy_lexical_root = root / "compose-cleanup-legacy-lexical"
        write_proof_compose_state(
            legacy_lexical_root,
            {"lexical_data_dir": None, "zoekt_data_dir": str(legacy_lexical_root / "lexical")},
        )
        cleanup_proof_cache(
            fake_cli,
            root,
            legacy_lexical_root,
            root / "compose-cleanup-legacy-lexical.json",
            successful_compose_cleanup,
        )

        failed_compose_root = root / "compose-cleanup-failed"
        write_proof_compose_state(failed_compose_root)
        failed_compose_artifact = root / "compose-cleanup-failed.json"

        def failed_compose_cleanup(command, **kwargs):
            if command[0] == "docker":
                return subprocess.CompletedProcess(command, 17, "", "forced failure")
            return subprocess.run(command, **kwargs)

        try:
            cleanup_proof_cache(
                fake_cli,
                root,
                failed_compose_root,
                failed_compose_artifact,
                failed_compose_cleanup,
            )
        except RuntimeError as exc:
            assert "Compose cleanup exited 17" in str(exc)
        else:
            raise AssertionError("failed proof-owned Compose cleanup must remain fail-closed")
        assert failed_compose_root.exists()
        assert read_json_file(failed_compose_artifact)["removed"] is False
        remove_tree_with_retry(failed_compose_root)

        masked_failure_root = root / "compose-cleanup-primary-failure"
        write_proof_compose_state(masked_failure_root)
        masked_failure_artifact = root / "compose-cleanup-primary-failure.json"
        primary_artifact = root / "primary-failure.json"
        try:
            with contextlib.ExitStack() as stack:
                stack.push(
                    cleanup_proof_cache_on_exit(
                        fake_cli,
                        root,
                        masked_failure_root,
                        masked_failure_artifact,
                        failed_compose_cleanup,
                    )
                )
                raise GateFailure("primary", primary_artifact, "primary gate failed")
        except GateFailure as exc:
            assert exc.layer == "primary"
            assert any("proof cleanup also failed" in note for note in getattr(exc, "__notes__", []))
        else:
            raise AssertionError("proof cleanup must not mask the primary gate failure")
        assert read_json_file(masked_failure_artifact)["removed"] is False
        remove_tree_with_retry(masked_failure_root)

        altered_compose_file = root / "altered-compose.yml"
        altered_compose_file.write_text("services: {}\n", encoding="utf-8")
        altered_qdrant = root / "altered-qdrant"
        altered_qdrant.mkdir()
        validation_cases = [
            ("project", {"compose_project": "not-codestory"}),
            ("file", {"compose_file": str(altered_compose_file)}),
            ("path", {"qdrant_data_dir": str(altered_qdrant)}),
            ("lexical-path", {"lexical_data_dir": str(altered_qdrant)}),
            ("non-string", {"namespace": 7}),
            ("lexical-non-string", {"lexical_data_dir": 7}),
        ]
        for label, overrides in validation_cases:
            cache_root = root / f"compose-validation-{label}"
            write_proof_compose_state(cache_root, overrides)
            artifact = root / f"compose-validation-{label}.json"
            try:
                cleanup_proof_cache(fake_cli, root, cache_root, artifact)
            except RuntimeError:
                pass
            else:
                raise AssertionError(f"altered proof Compose {label} must fail closed")
            validation = read_json_file(artifact)
            assert validation["removed"] is False
            assert validation["commands"][0]["kind"] == "compose_state_validation"
            assert validation["commands"][0]["error"]
            remove_tree_with_retry(cache_root)

        malformed_root = root / "compose-validation-malformed"
        malformed_state = write_proof_compose_state(malformed_root)
        malformed_state.write_text("{", encoding="utf-8")
        malformed_artifact = root / "compose-validation-malformed.json"
        try:
            cleanup_proof_cache(fake_cli, root, malformed_root, malformed_artifact)
        except RuntimeError:
            pass
        else:
            raise AssertionError("malformed proof Compose state must fail closed")
        assert "JSONDecodeError" in read_json_file(malformed_artifact)["commands"][0]["error"]
        remove_tree_with_retry(malformed_root)

        unreadable_root = root / "compose-validation-unreadable"
        write_proof_compose_state(unreadable_root)
        unreadable_artifact = root / "compose-validation-unreadable.json"

        def unreadable_state(_path):
            raise PermissionError("forced unreadable state")

        try:
            cleanup_proof_cache(
                fake_cli,
                root,
                unreadable_root,
                unreadable_artifact,
                read_state=unreadable_state,
            )
        except RuntimeError:
            pass
        else:
            raise AssertionError("unreadable proof Compose state must fail closed")
        assert "PermissionError" in read_json_file(unreadable_artifact)["commands"][0]["error"]
        remove_tree_with_retry(unreadable_root)

        symlink_root = root / "compose-validation-symlink"
        symlink_root.mkdir()
        (symlink_root / "qdrant").mkdir()
        (symlink_root / "lexical").mkdir()
        symlink_target = root / "compose-validation-symlink-target.json"
        write_json(symlink_target, {"owner": "codestory"})
        (symlink_root / "retrieval-sidecars.json").symlink_to(symlink_target)
        symlink_artifact = root / "compose-validation-symlink.json"
        try:
            cleanup_proof_cache(fake_cli, root, symlink_root, symlink_artifact)
        except RuntimeError:
            pass
        else:
            raise AssertionError("symlinked proof Compose state must fail closed")
        assert "symlink" in read_json_file(symlink_artifact)["commands"][0]["error"]
        remove_tree_with_retry(symlink_root)
        symlink_target.unlink()

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
        assert read_json_file(out_dir / "proof-cache-cleanup.json")["removed"] is True
        assert read_json_file(out_dir / "stdio-cache-cleanup.json")["removed"] is True
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
        assert version_only_summary["artifacts"].keys() == {
            "checksum",
            "version",
            "help",
            "serve_stdio",
            "proof_cache_cleanup",
            "stdio_cache_cleanup",
        }
        assert read_json_file(Path(version_only_args.out_dir) / "proof-cache-cleanup.json")["removed"] is True
        assert read_json_file(Path(version_only_args.out_dir) / "stdio-cache-cleanup.json")["removed"] is True
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
            assert delayed_status_worker.poll() is None
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
            assert retry_timeout_worker.poll() is None
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
        os.environ["CODESTORY_FAKE_LIST_CHANGED"] = "1"
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
            worker_cleanup = read_json_file(
                Path(managed_args.out_dir) / "managed-plugin-handoff-worker-cleanup.json"
            )["workers"]
            assert worker_cleanup[0]["source"] == "proof_started_repair_response"
            assert worker_cleanup[0]["attempt_id"] == "fake-attempt"
            assert worker_cleanup[0]["status"] in {
                "already_exited",
                "terminated",
                "terminated_after_taskkill",
                "terminated_after_direct_termination",
            }
            managed_cache_root = read_json_file(Path(managed_args.out_dir) / "managed-local-ground.json")["cache_root"]
            assert Path(managed_cache_root).name.startswith("codestory-packaged-proof-cache-")
            assert managed_status_after["cache_root"] == managed_cache_root
            managed_direct_status = read_json_file(Path(managed_args.out_dir) / "serve-stdio-status.json")["status"]
            assert Path(managed_direct_status["cache_root"]).name.startswith("codestory-packaged-stdio-cache-")
            assert managed_direct_status["cache_root"] != managed_cache_root
        finally:
            os.environ.pop("CODESTORY_FAKE_PLUGIN_MANAGED", None)
            os.environ.pop("CODESTORY_FAKE_LIST_CHANGED", None)
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

        fake_cli = stage / ("codestory-cli.cmd" if os.name == "nt" else "codestory-cli")
        numeric_id_artifact = root / "stdio-numeric-id.json"
        stdio_status_command(
            [str(fake_cli), "serve", "--stdio", "--refresh", "none", "--project", str(project)],
            numeric_id_artifact,
            2,
            project,
            extra_requests=[{"jsonrpc": "2.0", "id": 1, "method": "resources/list"}],
        )
        numeric_id_payload = read_json_file(numeric_id_artifact)
        assert any(
            entry.get("request", {}).get("id") == 1
            and entry.get("response", {}).get("id") == 1
            for entry in numeric_id_payload["transcript"]
        )

        for mode in (
            "wrong_id",
            "error",
            "out_of_order",
            "non_object",
            "server_request_collision",
            "malformed_jsonrpc",
            "malformed_method",
            "malformed_tools",
        ):
            protocol_artifact = root / f"stdio-{mode}.json"
            try:
                stdio_status(
                    fake_cli,
                    project,
                    protocol_artifact,
                    2,
                    {**os.environ, "CODESTORY_FAKE_INITIALIZE_MODE": mode},
                )
            except GateFailure as exc:
                assert exc.layer == "serve_stdio"
                assert exc.artifact == protocol_artifact
                assert protocol_artifact.is_file()
                protocol_stderr = protocol_artifact.with_suffix(protocol_artifact.suffix + ".stderr.txt")
                assert protocol_stderr.is_file()
                if mode == "malformed_jsonrpc":
                    assert "synthetic protocol stderr" in protocol_stderr.read_text(encoding="utf-8")
            else:
                raise AssertionError(f"{mode} stdio response must fail correlation")

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
