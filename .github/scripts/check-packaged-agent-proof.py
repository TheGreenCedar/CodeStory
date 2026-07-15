#!/usr/bin/env python3
"""Verify that a packaged CodeStory executable owns retrieval end to end.

The proof is deliberately process-shaped: one packaged executable, an embedded
model, an in-process engine, and no network or helper process preparation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zipfile
from pathlib import Path


STATUS_URI = "codestory://status"
ENGINE_DIAGNOSTICS_URI = "codestory://diagnostics/retrieval-engine"
DEFAULT_QUERY = "RuntimeContext"
DEFAULT_QUESTION = "Explain how CodeStory prepares retrieval."
SOFTWARE_ADAPTERS = ("llvmpipe", "lavapipe", "warp", "software rasterizer", "swiftshader")
LEGACY_TOKENS = (
    "llama-server",
    "repair-worker",
    "port-allocations",
    "native-embedding",
    "retrieval-sidecars",
    "sidecars",
)
LEGACY_HELP_TOKENS = ("llama-server", "sidecar", "repair", "consent", "download")


class ProofFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProofFailure(message)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def expected_archive_digest(checksum_file: Path, archive: Path) -> str:
    lines = checksum_file.read_text(encoding="utf-8").splitlines()
    records: dict[str, str] = {}
    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 2 and len(parts[0]) == 64:
            records[parts[-1].lstrip("*")] = parts[0].lower()
        elif len(parts) == 1 and len(parts[0]) == 64:
            records[archive.name] = parts[0].lower()
    require(archive.name in records, f"checksum file does not name {archive.name}")
    return records[archive.name]


def safe_target(root: Path, name: str) -> Path:
    target = (root / name).resolve()
    require(target.is_relative_to(root.resolve()), f"archive member escapes extraction root: {name}")
    return target


def unpack_archive(archive: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                safe_target(destination, member.filename)
            handle.extractall(destination)
        return
    if tarfile.is_tarfile(archive):
        with tarfile.open(archive) as handle:
            members = handle.getmembers()
            for member in members:
                safe_target(destination, member.name)
                require(not member.issym() and not member.islnk(), f"archive contains link: {member.name}")
            handle.extractall(destination, members=members)
        return
    raise ProofFailure(f"unsupported archive format: {archive}")


def find_cli(root: Path) -> Path:
    names = {"codestory-cli", "codestory-cli.exe"}
    matches = [path for path in root.rglob("*") if path.is_file() and path.name in names]
    require(len(matches) == 1, f"archive must contain exactly one native CodeStory executable; found {len(matches)}")
    cli = matches[0]
    cli.chmod(cli.stat().st_mode | stat.S_IXUSR)
    return cli


def run(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> dict:
    started = time.perf_counter()
    completed = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout)
    result = {
        "command": command,
        "exit_code": completed.returncode,
        "wall_ms": round((time.perf_counter() - started) * 1000, 3),
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    require(completed.returncode == 0, f"command failed ({completed.returncode}): {' '.join(command)}\n{completed.stderr[-2000:]}")
    return result


def json_command(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> tuple[dict, dict]:
    result = run(command, env=env, cwd=cwd, timeout=timeout)
    try:
        payload = json.loads(result["stdout"])
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"command did not emit JSON: {' '.join(command)}: {exc}") from exc
    require(isinstance(payload, dict), f"command emitted non-object JSON: {' '.join(command)}")
    return result, payload


def find_value(value: object, key: str) -> object | None:
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for child in value.values():
            found = find_value(child, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_value(child, key)
            if found is not None:
                return found
    return None


def engine_identity(status: dict, expected_policy: str | None, expected_backend: str | None) -> dict:
    fields = {
        key: find_value(status, key)
        for key in (
            "embedding_model_sha256",
            "embedding_ggml_build_identity",
            "embedding_backend",
            "embedding_adapter",
            "embedding_policy",
            "embedding_engine_instance_id",
            "embedding_model_load_count",
            "embedding_smoke_ms",
            "embedding_initialization_ms",
            "embedding_materialized_path",
            "embedding_materialized_reused",
            "embedding_accelerator_execution_verified",
            "embedding_model_layer_count",
            "embedding_offloaded_layer_count",
        )
    }
    digest = str(fields["embedding_model_sha256"] or "")
    require(len(digest) == 64 and all(char in "0123456789abcdefABCDEF" for char in digest), "status lacks an exact model digest")
    require(bool(fields["embedding_ggml_build_identity"]), "status lacks the linked ggml build identity")
    require(bool(fields["embedding_backend"]), "status lacks the selected embedding backend")
    adapter = str(fields["embedding_adapter"] or "")
    require(adapter, "status lacks the physical adapter identity")
    require(not any(token in adapter.lower() for token in SOFTWARE_ADAPTERS), f"software adapter is not allowed: {adapter}")
    require(fields["embedding_policy"] in {"accelerated", "cpu_explicit"}, "status lacks an explicit embedding policy")
    require(bool(fields["embedding_engine_instance_id"]), "status lacks the process engine identity")
    require(fields["embedding_model_load_count"] == 1, "the process must load one shared embedding model")
    require(isinstance(fields["embedding_smoke_ms"], (int, float)) and fields["embedding_smoke_ms"] >= 0, "status lacks the timed live embedding smoke")
    require(isinstance(fields["embedding_initialization_ms"], (int, float)) and fields["embedding_initialization_ms"] >= 0, "status lacks initialization timing")
    if expected_policy:
        require(fields["embedding_policy"] == expected_policy, f"embedding policy is {fields['embedding_policy']!r}, expected {expected_policy!r}")
    if expected_backend:
        observed = str(fields["embedding_backend"] or "").lower()
        expected = expected_backend.lower()
        matches = expected in observed or (expected == "metal" and observed == "mtl")
        require(matches, f"embedding backend is {fields['embedding_backend']!r}, expected {expected_backend!r}")
    if fields["embedding_policy"] == "accelerated":
        require(fields["embedding_accelerator_execution_verified"] is True, "accelerated policy lacks live accelerator execution proof")
        model_layers = fields["embedding_model_layer_count"]
        offloaded_layers = fields["embedding_offloaded_layer_count"]
        require(isinstance(model_layers, int) and model_layers > 0, "status lacks model layer count")
        require(offloaded_layers == model_layers, "not every model layer was offloaded")
    return fields


def assert_public_status(status: dict) -> None:
    require(find_value(status, "retrieval_mode") == "full", "public status does not report full retrieval")
    maintainer_only = (
        "sidecar",
        "full_repair",
        "embedding_model_sha256",
        "embedding_ggml_build_identity",
        "embedding_backend",
        "embedding_adapter",
        "embedding_policy",
        "embedding_engine_instance_id",
        "embedding_materialized_path",
        "embedding_detected_provider",
        "embedding_detected_gpu",
    )
    leaked = [key for key in maintainer_only if find_value(status, key) is not None]
    require(not leaked, "public status leaked maintainer-only retrieval fields: " + ", ".join(leaked))


def extract_resource(response: dict, uri: str) -> dict:
    require("error" not in response, f"status resource failed: {response.get('error')}")
    contents = response.get("result", {}).get("contents", [])
    for item in contents:
        if isinstance(item, dict) and item.get("uri") == uri:
            payload = json.loads(item.get("text", "{}"))
            require(isinstance(payload, dict), "status resource emitted non-object JSON")
            return payload
    raise ProofFailure(f"resource response did not contain {uri}")


class McpProcess:
    def __init__(self, command: list[str], *, env: dict[str, str], cwd: Path, timeout: int):
        self.timeout = timeout
        self.process = subprocess.Popen(command, cwd=cwd, env=env, text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.stderr: list[str] = []
        assert self.process.stdout and self.process.stderr and self.process.stdin
        threading.Thread(target=self._reader, args=(self.process.stdout, self.lines), daemon=True).start()
        threading.Thread(target=self._stderr_reader, daemon=True).start()
        self.transcript: list[dict] = []

    @staticmethod
    def _reader(stream, output: queue.Queue[str | None]) -> None:
        for line in stream:
            output.put(line)
        output.put(None)

    def _stderr_reader(self) -> None:
        assert self.process.stderr
        self.stderr.extend(self.process.stderr.readlines())

    def send(self, request: dict) -> dict:
        assert self.process.stdin
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"MCP request timed out: {request.get('id')}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty as exc:
                raise ProofFailure(f"MCP request timed out: {request.get('id')}") from exc
            require(line is not None, f"MCP process closed: {''.join(self.stderr)[-2000:]}")
            response = json.loads(line)
            self.transcript.append({"request": request, "response": response})
            if response.get("id") == request.get("id"):
                return response

    def initialize(self) -> None:
        response = self.send({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "packaged-proof", "version": "1"}},
        })
        require("error" not in response, f"MCP initialize failed: {response.get('error')}")
        assert self.process.stdin
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.process.stdin.flush()

    def status(self, project: Path, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": STATUS_URI, "project": str(project)},
        }), STATUS_URI)

    def engine_diagnostics(self, project: Path, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": ENGINE_DIAGNOSTICS_URI, "project": str(project)},
        }), ENGINE_DIAGNOSTICS_URI)

    def tool(self, name: str, arguments: dict, request_id: str) -> dict:
        response = self.send({"jsonrpc": "2.0", "id": request_id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
        require("error" not in response, f"MCP {name} failed: {response.get('error')}")
        return response

    def tool_until_ready(self, name: str, arguments: dict, request_id: str) -> tuple[dict, int]:
        deadline = time.monotonic() + self.timeout
        attempt = 0
        while True:
            attempt += 1
            response = self.tool(name, arguments, f"{request_id}-{attempt}")
            result = response.get("result", {})
            if result.get("isError") is not True:
                return response, attempt
            state = result.get("structuredContent", {})
            require(
                state.get("code") in {"codestory_preparing", "codestory_updating"},
                f"MCP {name} did not converge: {state}",
            )
            require(state.get("retry_tool") == name, f"MCP {name} returned the wrong retry tool: {state}")
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"MCP {name} did not become ready")
            delay = max(1, int(state.get("retry_after_ms", 1))) / 1000
            time.sleep(min(delay, remaining))

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


def isolated_environment(root: Path, policy: str | None, offline: bool) -> dict[str, str]:
    env = dict(os.environ)
    home = root / "home"
    cache = root / "cache"
    data = root / "plugin-data"
    temp = root / "tmp"
    for path in (home, cache, data, temp):
        path.mkdir(parents=True, exist_ok=True)
    env.update({
        "HOME": str(home),
        "USERPROFILE": str(home),
        "CODESTORY_CACHE_ROOT": str(cache),
        "CODESTORY_PLUGIN_DATA": str(data),
        "TMPDIR": str(temp),
        "TEMP": str(temp),
        "TMP": str(temp),
        "CODESTORY_EMBED_ALLOW_CPU": "1" if policy == "cpu_explicit" else "0",
    })
    if offline:
        env.update({
            "HTTP_PROXY": "http://127.0.0.1:1",
            "HTTPS_PROXY": "http://127.0.0.1:1",
            "ALL_PROXY": "http://127.0.0.1:1",
            "NO_PROXY": "",
            "CODESTORY_PLUGIN_DISABLE_PROVISION": "1",
        })
    for key in list(env):
        if key.startswith("CODESTORY_EMBED_") and key != "CODESTORY_EMBED_ALLOW_CPU":
            del env[key]
    return env


def assert_no_legacy_state(cache_root: Path) -> None:
    offenders = []
    for path in cache_root.rglob("*"):
        lowered = path.name.lower()
        if any(token in lowered for token in LEGACY_TOKENS) or path.suffix.lower() == ".pid":
            offenders.append(str(path))
    require(not offenders, "legacy process state was created: " + ", ".join(offenders[:10]))


def create_second_repository(root: Path) -> Path:
    repo = root / "second-repository"
    repo.mkdir()
    (repo / "README.md").write_text("# Second repository\n\nA tiny warm-engine reuse fixture.\n", encoding="utf-8")
    (repo / "lib.rs").write_text("pub fn shared_engine_probe() -> &'static str { \"warm\" }\n", encoding="utf-8")
    return repo


def prove_runtime(args: argparse.Namespace, cli: Path, env: dict[str, str], root: Path, out_dir: Path) -> dict:
    project = args.project.resolve()
    if args.additional_project:
        require(
            len(args.additional_project) == len(args.additional_query),
            "each --additional-project requires one --additional-query",
        )
        additional_projects = [
            (path.resolve(), query)
            for path, query in zip(args.additional_project, args.additional_query)
        ]
    else:
        additional_projects = [(create_second_repository(root), "shared_engine_probe")]
    embedded_models = Path(env["CODESTORY_CACHE_ROOT"]) / "embedded-models"
    require(not embedded_models.exists(), "isolated proof cache was not empty before first use")

    command = [str(cli), "serve", "--stdio", "--multi-project", "--refresh", "none"]
    if args.plugin_handoff:
        require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
        launcher = args.plugin_root.resolve() / "scripts" / "codestory-mcp.cjs"
        require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
        env["CODESTORY_CLI"] = str(cli)
        command = [shutil.which("node") or "node", str(launcher)]
    mcp = McpProcess(command, env=env, cwd=project, timeout=args.timeout_secs)
    try:
        mcp.initialize()
        cold_started = time.perf_counter()
        _, cold_attempts = mcp.tool_until_ready(
            "ground", {"project": str(project), "budget": "strict"}, "cold-ground"
        )
        cold_ground_wall_ms = round((time.perf_counter() - cold_started) * 1000, 3)
        cold_models = list(embedded_models.rglob("*.gguf"))
        require(len(cold_models) == 1, "first use did not materialize exactly one embedded model")
        cold_materialization = {
            "path": str(cold_models[0]),
            "sha256": sha256(cold_models[0]),
            "first_command_wall_ms": cold_ground_wall_ms,
            "ground_attempts": cold_attempts,
        }
        mcp.tool_until_ready(
            "search", {"project": str(project), "query": args.query, "why": True}, "search-one"
        )
        public_status = mcp.status(project, "status-one")
        assert_public_status(public_status)
        first = mcp.engine_diagnostics(project, "diagnostics-one")
        first_identity = engine_identity(first, args.engine_policy, args.expected_backend)
        for index, (additional_project, query) in enumerate(additional_projects, start=1):
            mcp.tool_until_ready(
                "search",
                {"project": str(additional_project), "query": query, "why": True},
                f"search-additional-{index}",
            )
            additional_status = mcp.engine_diagnostics(
                additional_project, f"diagnostics-additional-{index}"
            )
            additional_identity = engine_identity(
                additional_status, args.engine_policy, args.expected_backend
            )
            require(
                first_identity["embedding_engine_instance_id"]
                == additional_identity["embedding_engine_instance_id"],
                "repositories did not share one process engine",
            )
            require(
                additional_identity["embedding_model_load_count"] == 1,
                "an additional repository reloaded the model",
            )
        mcp.tool_until_ready(
            "packet", {"project": str(project), "question": args.question, "budget": "compact"}, "packet"
        )
        transcript = mcp.transcript
    finally:
        mcp.close()
    write_json(out_dir / "multi-repository-mcp.json", transcript)
    identity = first_identity

    materialized = Path(str(identity["embedding_materialized_path"] or ""))
    require(materialized.is_file(), f"materialized model is missing: {materialized}")
    require(sha256(materialized) == identity["embedding_model_sha256"], "materialized model digest does not match the embedded model")
    require(cold_materialization["sha256"] == identity["embedding_model_sha256"], "first-use model digest does not match engine identity")
    before_mtime = materialized.stat().st_mtime_ns

    restart = McpProcess([str(cli), "serve", "--stdio", "--multi-project", "--refresh", "none"], env=env, cwd=project, timeout=args.timeout_secs)
    try:
        restart.initialize()
        restart.tool("search", {"project": str(project), "query": args.query, "why": True}, "restart-search")
        restart_status = restart.engine_diagnostics(project, "restart-diagnostics")
        restart_identity = engine_identity(restart_status, args.engine_policy, args.expected_backend)
    finally:
        restart.close()
    require(Path(str(restart_identity["embedding_materialized_path"])).resolve() == materialized.resolve(), "restart used a different materialized model")
    require(restart_identity["embedding_materialized_reused"] is True, "restart did not report content-addressed model reuse")
    require(materialized.stat().st_mtime_ns == before_mtime, "restart rewrote the materialized model")
    assert_no_legacy_state(Path(env["CODESTORY_CACHE_ROOT"]))
    return {
        "cold_materialization": cold_materialization,
        "identity": identity,
        "restart_identity": restart_identity,
    }


def self_test() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        payload = root / "artifact.zip"
        with zipfile.ZipFile(payload, "w") as handle:
            handle.writestr("codestory-cli", b"binary")
        checksum = root / "SHA256SUMS.txt"
        checksum.write_text(f"{sha256(payload)}  {payload.name}\n", encoding="utf-8")
        require(expected_archive_digest(checksum, payload) == sha256(payload), "checksum parser failed")
        unpacked = root / "unpacked"
        unpack_archive(payload, unpacked)
        require(find_cli(unpacked).name == "codestory-cli", "CLI discovery failed")
        malicious = root / "malicious.zip"
        with zipfile.ZipFile(malicious, "w") as handle:
            handle.writestr("../outside", b"bad")
        try:
            unpack_archive(malicious, root / "bad")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("archive traversal was accepted")
        valid = {
            "embedding_model_sha256": "a" * 64,
            "embedding_ggml_build_identity": "ggml:test",
            "embedding_backend": "Metal",
            "embedding_adapter": "Apple GPU",
            "embedding_policy": "accelerated",
            "embedding_engine_instance_id": "engine-1",
            "embedding_model_load_count": 1,
            "embedding_smoke_ms": 1.0,
            "embedding_initialization_ms": 2.0,
            "embedding_accelerator_execution_verified": True,
            "embedding_model_layer_count": 13,
            "embedding_offloaded_layer_count": 13,
        }
        engine_identity(valid, "accelerated", "Metal")
        invalid = {**valid, "embedding_adapter": "llvmpipe"}
        try:
            engine_identity(invalid, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("software adapter was accepted")
    print("packaged in-process embedding proof self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--checksum-file", type=Path)
    parser.add_argument("--expected-version")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--plugin-root", type=Path)
    parser.add_argument("--out-dir", type=Path, default=Path("target/packaged-agent-proof"))
    parser.add_argument("--query", default=DEFAULT_QUERY)
    parser.add_argument("--question", default=DEFAULT_QUESTION)
    parser.add_argument("--additional-project", type=Path, action="append", default=[])
    parser.add_argument("--additional-query", action="append", default=[])
    parser.add_argument("--timeout-secs", type=int, default=900)
    parser.add_argument("--version-only", action="store_true")
    parser.add_argument("--plugin-handoff", action="store_true")
    parser.add_argument("--engine-policy", choices=("accelerated", "cpu_explicit"))
    parser.add_argument("--expected-backend")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    require(args.archive and args.checksum_file and args.expected_version, "archive, checksum, and expected version are required")
    args.archive = args.archive.resolve()
    args.checksum_file = args.checksum_file.resolve()
    args.out_dir = args.out_dir.resolve()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    require(sha256(args.archive) == expected_archive_digest(args.checksum_file, args.archive), "archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-") as raw:
        root = Path(raw)
        unpack_archive(args.archive, root / "unpacked")
        cli = find_cli(root / "unpacked")
        env = isolated_environment(root, args.engine_policy, args.offline)
        version = run([str(cli), "--version"], env=env, cwd=root, timeout=args.timeout_secs)
        require(args.expected_version in version["stdout"], f"CLI version does not contain {args.expected_version}")
        help_result = run([str(cli), "--help"], env=env, cwd=root, timeout=args.timeout_secs)
        help_text = help_result["stdout"].lower()
        require(
            not any(token in help_text for token in LEGACY_HELP_TOKENS),
            "top-level help exposes deleted embedding lifecycle terminology",
        )
        summary: dict[str, object] = {"version": version, "help": help_result}
        if not args.version_only:
            require(args.project is not None, "--project is required for the runtime proof")
            summary["runtime"] = prove_runtime(args, cli, env, root, args.out_dir)
        write_json(args.out_dir / "summary.json", summary)
    print(f"packaged CodeStory proof passed: {args.out_dir / 'summary.json'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ProofFailure, subprocess.TimeoutExpired, OSError, json.JSONDecodeError) as exc:
        print(f"packaged CodeStory proof failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
