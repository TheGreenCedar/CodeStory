#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import ctypes
import datetime
import hashlib
import http.server
import json
import os
import queue
import signal
import shutil
import socket
import stat
import struct
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
PROOF_TEMP_OWNER_FILE = ".codestory-macos-metal-proof-owner.json"
PROOF_LOCAL_RUN_ID = "shared-agent"
PROOF_AGENT_RUN_IDS = (PROOF_LOCAL_RUN_ID,)


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


def bounded_file_copy(source: Path, destination: Path, max_bytes: int = 4 * 1024 * 1024) -> dict:
    size = source.stat().st_size
    destination.parent.mkdir(parents=True, exist_ok=True)
    if size <= max_bytes:
        shutil.copyfile(source, destination)
        copied = size
        truncated = False
    else:
        half = max_bytes // 2
        with source.open("rb") as handle:
            head = handle.read(half)
            handle.seek(max(0, size - half))
            tail = handle.read(half)
        marker = f"\n--- CodeStory proof omitted {size - len(head) - len(tail)} log bytes ---\n".encode()
        destination.write_bytes(head + marker + tail)
        copied = destination.stat().st_size
        truncated = True
    return {
        "source": str(source),
        "source_size_bytes": size,
        "source_sha256": sha256_file(source),
        "preserved": str(destination),
        "preserved_size_bytes": copied,
        "truncated": truncated,
        "max_source_bytes_preserved": max_bytes,
    }


def preserve_native_embedding_evidence(
    cache_root: Path,
    out_dir: Path,
    label: str,
    *,
    required: bool = False,
    exact_launch: dict | None = None,
) -> Path | None:
    state_file = cache_root / "retrieval-sidecars.json"
    metadata_artifact = out_dir / f"{label}-native-launch.json"
    launch = exact_launch
    if launch is None:
        if not state_file.is_file() or state_file.is_symlink():
            if required:
                raise RuntimeError(f"native proof state is missing or unsafe: {state_file}")
            return None
        state = read_json_file(state_file)
        if not isinstance(state, dict) or state.get("owner") != "codestory":
            if required:
                raise RuntimeError(f"native proof state is not CodeStory-owned: {state_file}")
            return None
        launch = state.get("embedding_launch")
    if not isinstance(launch, dict) or launch.get("launch_mode") != "native_spawned":
        if required:
            raise RuntimeError(f"native proof state has no native_spawned launch: {state_file}")
        return None
    log_value = launch.get("log_path")
    payload = {
        "cache_root": str(cache_root),
        "state_file": str(state_file) if exact_launch is None else None,
        "embedding_launch": launch,
    }
    if exact_launch is not None:
        snapshot = registered_native_process_snapshot(launch)
        payload["live_identity"] = snapshot
        fingerprint = launch.get("launch_fingerprint_sha256")
        if snapshot.get("status") != "matching" or not (
            isinstance(fingerprint, str) and len(fingerprint) == 64
        ):
            payload["error"] = "exact broker launch identity is not live or fingerprinted"
            write_json(metadata_artifact, payload)
            if required:
                raise RuntimeError(payload["error"])
            return metadata_artifact
    if not isinstance(log_value, str) or not log_value:
        payload["error"] = "native launch metadata has no log_path"
        write_json(metadata_artifact, payload)
        if required:
            raise RuntimeError(payload["error"])
        return metadata_artifact
    log_path = Path(log_value)
    if log_path.is_symlink() or not log_path.is_file():
        payload["error"] = f"native launch log is missing or unsafe: {log_path}"
        write_json(metadata_artifact, payload)
        if required:
            raise RuntimeError(payload["error"])
        return metadata_artifact
    canonical_log = log_path.resolve(strict=True)
    canonical_cache = cache_root.resolve(strict=True)
    if not canonical_log.is_relative_to(canonical_cache):
        payload["error"] = f"native launch log escaped proof cache: {canonical_log}"
        write_json(metadata_artifact, payload)
        if required:
            raise RuntimeError(payload["error"])
        return metadata_artifact
    payload["log"] = bounded_file_copy(
        canonical_log,
        out_dir / f"{label}-llama-server-native.log",
    )
    write_json(metadata_artifact, payload)
    return metadata_artifact


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
    if sys.platform.startswith("linux") and hasattr(os, "getuid") and hasattr(os, "getgid"):
        env["CODESTORY_QDRANT_USER"] = f"{os.getuid()}:{os.getgid()}"
        env["CODESTORY_QDRANT_SNAPSHOTS_PATH"] = "/qdrant/storage/snapshots"
    return env


def write_managed_convergence_fixture(project: Path) -> None:
    (project / "src").mkdir(parents=True)
    (project / "Cargo.toml").write_text(
        '[package]\nname = "managed-convergence-fixture"\nversion = "0.1.0"\nedition = "2024"\n',
        encoding="utf-8",
    )
    (project / "src" / "lib.rs").write_text(
        "pub fn complete_publication() -> &'static str { \"initial\" }\n",
        encoding="utf-8",
    )


def macos_arm64_backend(project: Path) -> dict:
    metadata = project / "crates" / "codestory-retrieval" / "assets" / "llama-sidecar-backends.json"
    payload = read_json_file(metadata)
    backends = payload.get("backends", []) if isinstance(payload, dict) else []
    backend = next(
        (
            item
            for item in backends
            if isinstance(item, dict) and item.get("id") == "macos-aarch64-metal"
        ),
        None,
    )
    if not isinstance(backend, dict):
        raise RuntimeError(f"managed macOS arm64 backend is missing: {metadata}")
    return backend


def seed_corrupt_managed_server(cache_root: Path, project: Path) -> dict:
    backend = macos_arm64_backend(project)
    relative = backend.get("managed_cache_rel_dir")
    executable_name = backend.get("executable_rel_path")
    if not isinstance(relative, str) or not isinstance(executable_name, str):
        raise RuntimeError("managed macOS backend has incomplete install metadata")
    install_dir = cache_root / Path(relative)
    executable = install_dir / executable_name
    executable.parent.mkdir(parents=True, exist_ok=True)
    executable.write_bytes(b"interrupted managed llama-server install\n")
    executable.chmod(executable.stat().st_mode | stat.S_IXUSR)
    return {
        "backend": backend["id"],
        "executable": str(executable),
        "expected_sha256": backend.get("executable_sha256"),
    }


def require_managed_server_repaired(seed: dict, artifact: Path) -> None:
    executable = Path(seed["executable"])
    expected = seed.get("expected_sha256")
    actual = sha256_file(executable) if executable.is_file() else None
    payload = {**seed, "actual_sha256": actual, "repaired": actual == expected}
    write_json(artifact, payload)
    require(
        isinstance(expected, str) and actual == expected,
        "native_corrupt_server_repair",
        artifact,
        "managed native server was not checksum-repaired after a partial install",
    )


def free_local_ports(count: int) -> list[int]:
    listeners = []
    try:
        for _ in range(count):
            listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            listener.bind(("127.0.0.1", 0))
            listeners.append(listener)
        return [listener.getsockname()[1] for listener in listeners]
    finally:
        for listener in listeners:
            listener.close()


def make_managed_convergence_fixture_stale(project: Path) -> None:
    time.sleep(0.05)
    (project / "src" / "lib.rs").write_text(
        "pub fn complete_publication() -> &'static str { \"refreshed\" }\n",
        encoding="utf-8",
    )
    (project / "src" / "new_after_publication.rs").write_text(
        "pub fn discovered_after_publication() {}\n",
        encoding="utf-8",
    )


def proof_ownership_snapshot(cache_root: Path) -> dict[str, str]:
    names = {
        "local-refresh.lock",
        "local-refresh-status.json",
        "ready-repair-enqueue.lock",
        "ready-repair-result.json",
        "ready-repair-status.json",
    }
    return {
        str(path.relative_to(cache_root)): sha256_file(path)
        for path in cache_root.rglob("*")
        if path.is_file() and path.name in names
    }


def fnv1a_hex(value: str) -> str:
    digest = 0xCBF29CE484222325
    for byte in os.fsencode(value):
        digest ^= byte
        digest = (digest * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{digest:016x}"


def proof_agent_identity(cache_root: Path, project: Path, run_id: str) -> dict[str, str]:
    canonical_cache = cache_root.resolve(strict=False)
    canonical_project = project.resolve(strict=False)
    namespace = f"codestory-agent-{fnv1a_hex(str(canonical_project))}-{run_id}"
    state_root = canonical_cache / "sidecars" / namespace
    return {
        "cache_root": str(canonical_cache),
        "project": str(canonical_project),
        "profile": "agent",
        "run_id": run_id,
        "namespace": namespace,
        "compose_project": namespace,
        "state_file": str(state_root / "retrieval-sidecars.json"),
        "qdrant_data_dir": str(state_root / "qdrant"),
        "lexical_data_dir": str(state_root / "lexical"),
        "scip_artifacts_root": str(state_root / "scip"),
    }


def proof_agent_identities(cache_roots: list[Path], project: Path) -> list[dict[str, str]]:
    return [
        proof_agent_identity(cache, project, run_id)
        for cache in cache_roots
        for run_id in PROOF_AGENT_RUN_IDS
    ]


def run_docker_json(
    command: list[str],
    run=subprocess.run,
    env: dict[str, str] | None = None,
) -> object:
    try:
        result = run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=20,
            check=False,
            text=True,
            env=env,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise RuntimeError(f"could not inspect Docker proof resources: {exc}") from exc
    if result.returncode != 0:
        raise RuntimeError(
            f"Docker proof resource inspection exited {result.returncode}: "
            f"{captured_text(result.stderr).strip()}"
        )
    body = captured_text(result.stdout).strip()
    if not body:
        return []
    try:
        return json.loads(body)
    except json.JSONDecodeError as aggregate_error:
        try:
            return [json.loads(line) for line in body.splitlines() if line.strip()]
        except json.JSONDecodeError as exc:
            raise RuntimeError(
                f"Docker proof resource inspection returned invalid JSON: {aggregate_error}; {exc}"
            ) from exc


def docker_created_epoch_ms(value: object) -> int | None:
    if not isinstance(value, str) or not value:
        return None
    candidate = value.strip().replace("Z", "+00:00")
    try:
        parsed = datetime.datetime.fromisoformat(candidate)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=datetime.timezone.utc)
    return int(parsed.timestamp() * 1000)


def docker_compose_resource_snapshot(state: dict, run=subprocess.run) -> dict:
    compose_project = state.get("compose_project")
    if not isinstance(compose_project, str) or not compose_project:
        raise RuntimeError("proof sidecar state has no Compose project for Docker inspection")
    inspect_env = {
        **os.environ,
        "CODESTORY_SIDECAR_NAMESPACE": str(state.get("namespace", "")),
        "CODESTORY_QDRANT_DATA_DIR": str(state.get("qdrant_data_dir", "")),
    }

    def matching_ids(kind: str) -> list[str]:
        payload = run_docker_json(
            [
                "docker",
                kind,
                "ls",
                "--all" if kind == "container" else "--filter",
                *(
                    ["--filter", f"label=com.docker.compose.project={compose_project}"]
                    if kind == "container"
                    else [f"label=com.docker.compose.project={compose_project}"]
                ),
                "--format",
                "{{json .}}",
            ],
            run,
            inspect_env,
        )
        # Docker 29 emits a single JSON object (not an array) when `ls --format
        # json` has exactly one match. Multiple matches remain newline-delimited
        # objects and are normalized by `run_docker_json`.
        if isinstance(payload, dict):
            payload = [payload]
        if not isinstance(payload, list):
            raise RuntimeError(f"Docker {kind} listing was not a JSON array")
        ids = []
        for item in payload:
            if not isinstance(item, dict):
                raise RuntimeError(f"Docker {kind} listing contained a non-object")
            value = item.get("ID")
            if not isinstance(value, str) or not value:
                raise RuntimeError(f"Docker {kind} listing contained no ID")
            ids.append(value)
        return sorted(set(ids))

    container_ids = matching_ids("container")
    network_ids = matching_ids("network")
    container_inspect = (
        run_docker_json(["docker", "container", "inspect", *container_ids], run, inspect_env)
        if container_ids
        else []
    )
    network_inspect = (
        run_docker_json(["docker", "network", "inspect", *network_ids], run, inspect_env)
        if network_ids
        else []
    )
    if not isinstance(container_inspect, list) or not isinstance(network_inspect, list):
        raise RuntimeError("Docker proof resource inspect output was not an array")

    containers = []
    for item in container_inspect:
        if not isinstance(item, dict):
            raise RuntimeError("Docker container inspect contained a non-object")
        config = item.get("Config") if isinstance(item.get("Config"), dict) else {}
        labels = config.get("Labels") if isinstance(config.get("Labels"), dict) else {}
        mounts = item.get("Mounts") if isinstance(item.get("Mounts"), list) else []
        containers.append(
            {
                "id": item.get("Id"),
                "name": str(item.get("Name", "")).lstrip("/"),
                "created": item.get("Created"),
                "labels": {str(key): str(value) for key, value in sorted(labels.items())},
                "mounts": sorted(
                    [
                        {
                            "type": mount.get("Type"),
                            "source": mount.get("Source"),
                            "destination": mount.get("Destination"),
                        }
                        for mount in mounts
                        if isinstance(mount, dict)
                    ],
                    key=lambda mount: (
                        str(mount.get("destination")),
                        str(mount.get("source")),
                    ),
                ),
            }
        )
    networks = []
    for item in network_inspect:
        if not isinstance(item, dict):
            raise RuntimeError("Docker network inspect contained a non-object")
        labels = item.get("Labels") if isinstance(item.get("Labels"), dict) else {}
        attached = item.get("Containers") if isinstance(item.get("Containers"), dict) else {}
        networks.append(
            {
                "id": item.get("Id"),
                "name": item.get("Name"),
                "created": item.get("Created"),
                "labels": {str(key): str(value) for key, value in sorted(labels.items())},
                "attached_container_ids": sorted(str(value) for value in attached),
            }
        )
    return {
        "compose_project": compose_project,
        "containers": sorted(containers, key=lambda item: (str(item.get("id")), str(item.get("name")))),
        "networks": sorted(networks, key=lambda item: (str(item.get("id")), str(item.get("name")))),
    }


def validate_proof_docker_resources(
    state: dict,
    observed: dict,
    registered: dict | None = None,
) -> None:
    if registered is not None:
        if registered.get("compose_project") != observed.get("compose_project"):
            raise RuntimeError("Docker resources changed their registered Compose project")
        for kind in ("containers", "networks"):
            registered_items = registered.get(kind)
            observed_items = observed.get(kind)
            if not isinstance(registered_items, list) or not isinstance(observed_items, list):
                raise RuntimeError("registered Docker resource snapshot is malformed")
            registered_by_id = {
                item.get("id"): item
                for item in registered_items
                if isinstance(item, dict) and isinstance(item.get("id"), str)
            }
            observed_by_id = {
                item.get("id"): item
                for item in observed_items
                if isinstance(item, dict) and isinstance(item.get("id"), str)
            }
            if len(registered_by_id) != len(registered_items) or len(observed_by_id) != len(
                observed_items
            ):
                raise RuntimeError("Docker resource snapshot contains duplicate or invalid IDs")
            if not set(observed_by_id).issubset(registered_by_id):
                raise RuntimeError("Docker resources include IDs absent from proof registration")
            for resource_id, item in observed_by_id.items():
                registered_item = registered_by_id[resource_id]
                if kind == "containers" and item != registered_item:
                    raise RuntimeError(
                        f"Docker container {resource_id} changed after proof ownership registration"
                    )
                if kind == "networks" and any(
                    item.get(field) != registered_item.get(field)
                    for field in ("id", "name", "created", "labels")
                ):
                    raise RuntimeError(
                        f"Docker network {resource_id} changed after proof ownership registration"
                    )
    compose_project = state["compose_project"]
    if observed.get("compose_project") != compose_project:
        raise RuntimeError("Docker resource snapshot changed the Compose project")
    containers = observed.get("containers")
    networks = observed.get("networks")
    if not isinstance(containers, list) or not isinstance(networks, list):
        raise RuntimeError("Docker resource snapshot is malformed")
    if not containers and networks and registered is None:
        raise RuntimeError("cannot prove an unregistered Compose network without its containers")
    expected_container_ids = set()
    services = set()
    qdrant_created = None
    for container in containers:
        if not isinstance(container, dict):
            raise RuntimeError("Docker resource snapshot contains a malformed container")
        container_id = container.get("id")
        labels = container.get("labels")
        if not isinstance(container_id, str) or not container_id or not isinstance(labels, dict):
            raise RuntimeError("Docker resource snapshot contains an unidentified container")
        expected_labels = {
            "com.docker.compose.project": compose_project,
            "dev.codestory.owner": "codestory",
            "dev.codestory.profile": "agent",
            "dev.codestory.namespace": state["namespace"],
        }
        if any(labels.get(name) != value for name, value in expected_labels.items()):
            raise RuntimeError(f"Docker container {container_id} has foreign ownership labels")
        service = labels.get("com.docker.compose.service")
        if service not in {"qdrant", "embed"} or service in services:
            raise RuntimeError(f"Docker container {container_id} has an unexpected Compose service")
        if container.get("name") != f"{state['namespace']}-{service}":
            raise RuntimeError(f"Docker container {container_id} has a foreign resource name")
        services.add(service)
        expected_container_ids.add(container_id)
        created = docker_created_epoch_ms(container.get("created"))
        if created is None:
            raise RuntimeError(f"Docker container {container_id} has no creation identity")
        if service == "qdrant":
            qdrant_created = created
            expected_qdrant = Path(state["qdrant_data_dir"]).resolve(strict=False)
            matching_mount = any(
                isinstance(mount, dict)
                and mount.get("type") == "bind"
                and mount.get("destination") == "/qdrant/storage"
                and isinstance(mount.get("source"), str)
                and Path(mount["source"]).resolve(strict=False) == expected_qdrant
                for mount in container.get("mounts", [])
            )
            if not matching_mount:
                raise RuntimeError(
                    f"Docker qdrant container {container_id} is mounted from a foreign cache"
                )
    if containers and "qdrant" not in services:
        raise RuntimeError("Docker proof resources have no cache-bound qdrant container")
    if qdrant_created is not None:
        started = state.get("started_at_epoch_ms")
        if isinstance(started, int) and not (qdrant_created - 5_000 <= started <= qdrant_created + 1_800_000):
            raise RuntimeError("Docker qdrant creation does not match the sidecar state lifetime")
        for container in containers:
            created = docker_created_epoch_ms(container.get("created"))
            if created is None or abs(created - qdrant_created) > 300_000:
                raise RuntimeError("Docker containers do not share the registered creation lifetime")
    if len(networks) > 1:
        raise RuntimeError("Docker proof resources contain multiple Compose networks")
    for network in networks:
        if not isinstance(network, dict):
            raise RuntimeError("Docker resource snapshot contains a malformed network")
        network_id = network.get("id")
        labels = network.get("labels")
        if not isinstance(network_id, str) or not network_id or not isinstance(labels, dict):
            raise RuntimeError("Docker resource snapshot contains an unidentified network")
        if (
            labels.get("com.docker.compose.project") != compose_project
            or labels.get("com.docker.compose.network") != "default"
            or network.get("name") != f"{compose_project}_default"
        ):
            raise RuntimeError(f"Docker network {network_id} has foreign ownership labels")
        attached = network.get("attached_container_ids")
        if not isinstance(attached, list) or not set(attached).issubset(expected_container_ids):
            raise RuntimeError(f"Docker network {network_id} has foreign attached containers")
        network_created = docker_created_epoch_ms(network.get("created"))
        if network_created is None:
            raise RuntimeError(f"Docker network {network_id} has no creation identity")
        if qdrant_created is not None and abs(network_created - qdrant_created) > 300_000:
            raise RuntimeError(f"Docker network {network_id} has a foreign creation lifetime")


def proof_temp_root_from_environment() -> Path | None:
    value = os.environ.get("CODESTORY_PROOF_TEMP_ROOT", "").strip()
    if not value:
        return None
    root = Path(value)
    if root.is_symlink() or not root.is_dir():
        raise RuntimeError(f"proof temp root is missing or unsafe: {root}")
    return root.resolve(strict=True)


def register_proof_temp_ownership(project: Path, cache_roots: list[Path], archive: Path) -> None:
    root = proof_temp_root_from_environment()
    if root is None:
        return
    canonical_project = project.resolve(strict=True)
    canonical_caches = []
    for cache in cache_roots:
        canonical = cache.resolve(strict=True)
        if not canonical.is_relative_to(root):
            raise RuntimeError(f"proof cache is outside registered proof temp root: {canonical}")
        canonical_caches.append(str(canonical))
    canonical_archive = archive.resolve(strict=True)
    owned_archive = root / canonical_archive.name
    if owned_archive.is_symlink() or not owned_archive.is_file():
        raise RuntimeError(f"proof-owned archive copy is missing or unsafe: {owned_archive}")
    if sha256_file(owned_archive) != sha256_file(canonical_archive):
        raise RuntimeError("proof-owned archive copy does not match the verified input archive")
    marker = root / PROOF_TEMP_OWNER_FILE
    payload = {
        "owner": "codestory-macos-metal-proof",
        "repository": os.environ.get("GITHUB_REPOSITORY"),
        "project": str(canonical_project),
        "cache_roots": canonical_caches,
        "sidecars": proof_agent_identities([Path(value) for value in canonical_caches], canonical_project),
        "launches": [],
        "ports": [],
        "archive_name": owned_archive.name,
        "archive_sha256": sha256_file(owned_archive),
        "harness_pid": os.getpid(),
        "created_at_epoch_ms": int(time.time() * 1000),
    }
    write_json(marker, payload)


def load_proof_temp_ownership() -> tuple[Path, dict] | None:
    root = proof_temp_root_from_environment()
    if root is None:
        return None
    marker = root / PROOF_TEMP_OWNER_FILE
    payload = read_json_file(marker)
    if not isinstance(payload, dict) or payload.get("owner") != "codestory-macos-metal-proof":
        raise RuntimeError(f"proof temp owner marker is invalid: {marker}")
    return marker, payload


def record_proof_runtime_identity(
    payload: dict,
    launch: dict | None,
    ports: list[object] | tuple[object, ...] | None = None,
) -> None:
    if isinstance(launch, dict) and launch.get("launch_mode") == "native_spawned":
        fingerprint = (launch.get("pid"), launch.get("launch_fingerprint_sha256"))
        existing = {
            (item.get("pid"), item.get("launch_fingerprint_sha256"))
            for item in payload.get("launches", [])
            if isinstance(item, dict)
        }
        if fingerprint not in existing:
            payload.setdefault("launches", []).append(launch)
    known_ports = {value for value in payload.get("ports", []) if isinstance(value, int)}
    for port in ports or ():
        if isinstance(port, int) and 0 < port <= 65535 and port not in known_ports:
            payload.setdefault("ports", []).append(port)
            known_ports.add(port)


def register_current_proof_runtime(cache_root: Path) -> None:
    ownership = load_proof_temp_ownership()
    if ownership is None:
        return
    marker, payload = ownership
    registered = payload.get("sidecars", [])
    if not isinstance(registered, list):
        raise RuntimeError(f"proof temp owner marker has invalid sidecars: {marker}")
    for identity in registered:
        if not isinstance(identity, dict):
            raise RuntimeError(f"proof temp owner marker has invalid sidecar identity: {marker}")
        if Path(str(identity.get("cache_root", ""))).resolve(strict=False) != cache_root.resolve(strict=True):
            continue
        validated = validated_proof_compose_state(
            cache_root,
            Path(identity.get("project", payload.get("project", "."))),
            read_json_file,
            run_id=str(identity.get("run_id", "")),
            registered_identity=identity,
        )
        if validated is None:
            continue
        _, state = validated
        if state.get("compose_file") is not None:
            snapshot = docker_compose_resource_snapshot(state)
            registered_resources = identity.get("docker_resources")
            validate_proof_docker_resources(
                state,
                snapshot,
                registered_resources if isinstance(registered_resources, dict) else None,
            )
            identity["docker_resources"] = snapshot
        record_proof_runtime_identity(
            payload,
            state.get("embedding_launch"),
            (
                state.get("qdrant_http_port"),
                state.get("qdrant_grpc_port"),
                state.get("embed_http_port"),
            ),
        )
    write_json(marker, payload)


def register_proof_launch(launch: dict | None, ports: list[object] | None = None) -> None:
    ownership = load_proof_temp_ownership()
    if ownership is None:
        return
    marker, payload = ownership
    record_proof_runtime_identity(payload, launch, ports)
    write_json(marker, payload)


def validated_proof_compose_state(
    cache_root: Path,
    project: Path,
    read_state,
    *,
    run_id: str = PROOF_LOCAL_RUN_ID,
    registered_identity: dict | None = None,
    allow_missing_compose_file: bool = False,
) -> tuple[Path, dict] | None:
    if cache_root.is_symlink() or project.is_symlink():
        raise RuntimeError("proof cache and project roots must not be symlinks")
    cache_root = cache_root.resolve(strict=True)
    project = project.resolve(strict=registered_identity is None)
    local_state = cache_root / "retrieval-sidecars.json"
    if local_state.exists() or local_state.is_symlink():
        raise RuntimeError(
            f"proof cleanup refuses the global local-sidecar namespace: {local_state}"
        )
    expected = registered_identity or proof_agent_identity(cache_root, project, run_id)
    required_identity = proof_agent_identity(cache_root, Path(expected.get("project", project)), run_id)
    for name in (
        "cache_root",
        "project",
        "profile",
        "run_id",
        "namespace",
        "compose_project",
        "state_file",
        "qdrant_data_dir",
        "lexical_data_dir",
        "scip_artifacts_root",
    ):
        if expected.get(name) != required_identity[name]:
            raise RuntimeError(f"registered proof sidecar identity changed {name!r}")
    if expected["namespace"] == "codestory" or not expected["namespace"].startswith("codestory-agent-"):
        raise RuntimeError("proof sidecar namespace is not isolated from the global local namespace")
    state_file = Path(expected["state_file"])
    canonical_state_file = state_file.resolve(strict=False)
    if (
        state_file.is_symlink()
        or canonical_state_file != state_file
        or not canonical_state_file.is_relative_to(cache_root)
    ):
        raise RuntimeError(f"proof sidecar state escaped its cache root through a symlink: {state_file}")
    if not state_file.exists():
        return None
    if not state_file.is_file():
        raise RuntimeError(f"proof sidecar state is not a regular file: {state_file}")
    state = read_state(state_file)
    if not isinstance(state, dict):
        raise TypeError(f"proof sidecar state is not an object: {state_file}")
    expected_identity = {
        "owner": "codestory",
        "profile": "agent",
        "run_id": run_id,
        "namespace": expected["namespace"],
        "compose_project": expected["compose_project"],
    }
    for name, expected_value in expected_identity.items():
        if state.get(name) != expected_value:
            raise RuntimeError(
                f"proof sidecar state {name} does not match {expected_value!r}: {state_file}"
            )
    state = dict(state)
    for name, expected_path in (
        ("qdrant_data_dir", Path(expected["qdrant_data_dir"])),
        ("scip_artifacts_root", Path(expected["scip_artifacts_root"])),
    ):
        value = state.get(name)
        if not isinstance(value, str) or not value:
            raise TypeError(f"proof sidecar state {name} is not a path string: {state_file}")
        observed = Path(value)
        canonical_expected = expected_path.resolve(strict=False)
        if (
            canonical_expected != expected_path
            or not canonical_expected.is_relative_to(cache_root)
            or observed.is_symlink()
            or observed.resolve(strict=False) != canonical_expected
        ):
            raise RuntimeError(f"proof sidecar state {name} escaped its cache root: {state_file}")
    lexical_expected = Path(expected["lexical_data_dir"])
    canonical_lexical_expected = lexical_expected.resolve(strict=False)
    if (
        canonical_lexical_expected != lexical_expected
        or not canonical_lexical_expected.is_relative_to(cache_root)
    ):
        raise RuntimeError(f"proof sidecar lexical_data_dir escaped its cache root: {state_file}")
    lexical_value = state.get("lexical_data_dir")
    if lexical_value is None:
        lexical_value = state.get("zoekt_data_dir")
    if not isinstance(lexical_value, str) or not lexical_value:
        raise TypeError(f"proof sidecar state canonical or legacy lexical data directory is not a path string: {state_file}")
    observed = Path(lexical_value)
    if observed.is_symlink() or observed.resolve(strict=False) != canonical_lexical_expected:
        raise RuntimeError(f"proof sidecar state lexical_data_dir escaped its cache root: {state_file}")
    state["lexical_data_dir"] = str(lexical_expected)
    state["proof_identity"] = expected
    compose_value = state.get("compose_file")
    if compose_value is None:
        state["compose_file"] = None
        return state_file, state
    if not isinstance(compose_value, str) or not compose_value:
        raise TypeError(f"proof sidecar compose_file is not a path string: {state_file}")
    compose_file = Path(compose_value)
    if compose_file.is_symlink():
        raise RuntimeError(f"proof sidecar compose file is not a regular canonical file: {compose_file}")
    allowed_compose_paths = {
        candidate.resolve(strict=False)
        for candidate in (
            Path(expected["project"]) / "docker" / "retrieval-compose.yml",
            cache_root / "retrieval-compose.yml",
        )
    }
    if not compose_file.is_file():
        if (
            allow_missing_compose_file
            and compose_file.resolve(strict=False) in allowed_compose_paths
        ):
            state["compose_file_missing"] = True
            return state_file, state
        raise RuntimeError(f"proof sidecar compose file is not a regular canonical file: {compose_file}")
    allowed_compose_files = set()
    for candidate in (
        Path(expected["project"]) / "docker" / "retrieval-compose.yml",
        cache_root / "retrieval-compose.yml",
    ):
        if not candidate.is_file() or candidate.is_symlink():
            continue
        canonical_candidate = candidate.resolve(strict=True)
        if candidate == canonical_candidate:
            allowed_compose_files.add(canonical_candidate)
    canonical_compose_file = compose_file.resolve(strict=True)
    if canonical_compose_file not in allowed_compose_files:
        raise RuntimeError(f"proof sidecar compose file is outside the allowed roots: {compose_file}")
    return state_file, state


def cleanup_proof_compose(
    cache_root: Path,
    project: Path,
    env: dict[str, str],
    results: list[dict],
    run,
    read_state,
    *,
    run_id: str = PROOF_LOCAL_RUN_ID,
    registered_identity: dict | None = None,
) -> None:
    try:
        validated = validated_proof_compose_state(
            cache_root,
            project,
            read_state,
            run_id=run_id,
            registered_identity=registered_identity,
            allow_missing_compose_file=True,
        )
    except Exception as exc:
        results.append(
            {
                "kind": "compose_state_validation",
                "state_file": str(
                    registered_identity.get("state_file")
                    if isinstance(registered_identity, dict)
                    else proof_agent_identity(cache_root, project, run_id)["state_file"]
                ),
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        raise RuntimeError(f"proof-owned Compose state validation failed: {exc}") from exc
    if validated is None:
        return
    state_file, state = validated
    if state.get("compose_file") is None:
        results.append(
            {
                "kind": "compose_down_skipped",
                "state_file": str(state_file),
                "proof_identity": state["proof_identity"],
                "reason": "sidecar_state_has_no_compose_file",
            }
        )
        return
    try:
        observed = docker_compose_resource_snapshot(state, run)
        registered_resources = (
            registered_identity.get("docker_resources")
            if isinstance(registered_identity, dict)
            else None
        )
        validate_proof_docker_resources(
            state,
            observed,
            registered_resources if isinstance(registered_resources, dict) else None,
        )
    except Exception as exc:
        results.append(
            {
                "kind": "docker_resource_validation",
                "state_file": str(state_file),
                "proof_identity": state["proof_identity"],
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        raise RuntimeError(f"proof-owned Docker resource validation failed: {exc}") from exc
    containers = [item["id"] for item in observed["containers"]]
    networks = [item["id"] for item in observed["networks"]]
    resource_env = {
        **env,
        "CODESTORY_SIDECAR_NAMESPACE": state["namespace"],
        "CODESTORY_QDRANT_DATA_DIR": state["qdrant_data_dir"],
    }
    for kind, ids in (("container", containers), ("network", networks)):
        if not ids:
            continue
        command = [
            "docker",
            kind,
            "rm",
            *(("-f",) if kind == "container" else ()),
            *ids,
        ]
        try:
            result = run(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=30,
                check=False,
                env=resource_env,
                text=True,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            results.append(
                {"kind": f"docker_{kind}_remove", "state_file": str(state_file), "error": str(exc)}
            )
            raise RuntimeError(f"could not remove exact proof-owned Docker {kind}s: {exc}") from exc
        results.append(
            {
                "kind": f"docker_{kind}_remove",
                "state_file": str(state_file),
                "proof_identity": state["proof_identity"],
                "resource_ids": ids,
                "returncode": result.returncode,
                "stdout": result.stdout,
                "stderr": result.stderr,
            }
        )
        if result.returncode != 0:
            raise RuntimeError(f"proof-owned Docker {kind} cleanup exited {result.returncode}")
    if not containers and not networks:
        results.append(
            {
                "kind": "docker_resources_absent",
                "state_file": str(state_file),
                "proof_identity": state["proof_identity"],
            }
        )


def cleanup_proof_cache(
    cli: Path | None,
    project: Path,
    cache_root: Path,
    artifact: Path,
    run=subprocess.run,
    read_state=read_json_file,
    *,
    registered_sidecars: list[dict] | None = None,
    direct_only: bool = False,
) -> None:
    del cli
    env = {**os.environ, "CODESTORY_CACHE_ROOT": str(cache_root)}
    results = []
    errors = []
    canonical_cache = cache_root.resolve(strict=True)
    identities = (
        registered_sidecars
        if registered_sidecars is not None
        else proof_agent_identities([canonical_cache], project)
    )
    identities = [
        identity
        for identity in identities
        if isinstance(identity, dict)
        and Path(str(identity.get("cache_root", ""))).resolve(strict=False) == canonical_cache
    ]
    expected_state_files = {
        Path(identity["state_file"]).resolve(strict=False)
        for identity in identities
        if isinstance(identity.get("state_file"), str)
    }
    discovered_state_files = {
        path.resolve(strict=False)
        for path in (canonical_cache / "sidecars").glob("*/retrieval-sidecars.json")
    }
    unexpected = sorted(str(path) for path in discovered_state_files - expected_state_files)
    if unexpected:
        error = f"proof cache contains unregistered sidecar state: {unexpected}"
        results.append({"kind": "compose_state_validation", "error": error})
        errors.append(error)
    validated_states: list[tuple[dict, dict]] = []
    for identity in identities:
        try:
            run_id = identity.get("run_id")
            if run_id not in PROOF_AGENT_RUN_IDS:
                raise RuntimeError(f"unapproved proof sidecar run id: {run_id!r}")
            validated = validated_proof_compose_state(
                canonical_cache,
                project,
                read_state,
                run_id=run_id,
                registered_identity=identity,
                allow_missing_compose_file=True,
            )
            if validated is None:
                continue
            _, state = validated
            validated_states.append((identity, state))
            cleanup_proof_compose(
                canonical_cache,
                project,
                env,
                results,
                run,
                read_state,
                run_id=run_id,
                registered_identity=identity,
            )
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            if not results or results[-1].get("error") != error:
                results.append(
                    {
                        "kind": "compose_state_validation",
                        "state_file": str(identity.get("state_file", "")),
                        "error": error,
                    }
                )
            errors.append(error)
    seen_launches = set()
    for _identity, state in validated_states:
        launch = state.get("embedding_launch")
        if (
            not isinstance(launch, dict)
            or state.get("embedding_launch_ownership", "owner") != "owner"
        ):
            continue
        fingerprint = (launch.get("pid"), launch.get("launch_fingerprint_sha256"))
        if fingerprint in seen_launches:
            continue
        seen_launches.add(fingerprint)
        try:
            results.append(
                {
                    "kind": "native_process_cleanup",
                    "result": terminate_registered_native_process(launch),
                }
            )
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            results.append(
                {
                    "kind": "native_process_cleanup",
                    "pid": launch.get("pid"),
                    "error": error,
                }
            )
            errors.append(error)
    results.append(
        {
            "kind": "retrieval_down_skipped",
            "reason": "trusted_direct_cleanup" if direct_only else "exact_registered_resource_cleanup",
            "validated_sidecars": [identity for identity, _ in validated_states],
        }
    )
    if errors:
        write_json(
            artifact,
            {
                "cache_root": str(cache_root),
                "commands": results,
                "removed": False,
                "errors": errors,
            },
        )
        raise RuntimeError(
            "proof-owned sidecar cleanup had failures after attempting every registered resource: "
            + "; ".join(errors)
        )
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
    registered_sidecars: list[dict] | None = None,
):
    def cleanup(exc_type, exc, traceback) -> bool:
        evidence_error = None
        try:
            preserve_native_embedding_evidence(cache_root, artifact.parent, "native-final")
        except Exception as preserve_exc:
            evidence_error = preserve_exc
        try:
            cleanup_proof_cache(
                cli,
                project,
                cache_root,
                artifact,
                run,
                read_state,
                registered_sidecars=registered_sidecars,
            )
        except Exception as cleanup_exc:
            if exc is None:
                raise
            if hasattr(exc, "add_note"):
                exc.add_note(f"proof cleanup also failed: {type(cleanup_exc).__name__}: {cleanup_exc}")
        if evidence_error is not None:
            if exc is None:
                raise evidence_error
            if hasattr(exc, "add_note"):
                exc.add_note(
                    f"native evidence preservation also failed: "
                    f"{type(evidence_error).__name__}: {evidence_error}"
                )
        return False

    return cleanup


def proof_agent_environment(base: dict[str, str], run_id: str) -> dict[str, str]:
    """Keep proof sidecars out of the process-global local Compose namespace."""
    return {
        **base,
        "CODESTORY_SIDECAR_PROFILE": "agent",
        "CODESTORY_SIDECAR_RUN_ID": run_id,
    }


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


def require_native_accelerator_ready(
    payload: object,
    layer: str,
    artifact: Path,
    expected_pid: int | None = None,
) -> dict:
    require_agent_ready(payload, layer, artifact)
    require(isinstance(payload, dict), layer, artifact, "ready output is not a JSON object")
    broker = payload.get("readiness_broker")
    require(isinstance(broker, dict), layer, artifact, "ready output missing readiness_broker")
    proof = broker.get("gpu_proof")
    require(isinstance(proof, dict), layer, artifact, "ready output missing gpu_proof")
    require(proof.get("proof_status") == "verified", layer, artifact, "native GPU proof is not verified")
    require(
        proof.get("meaningful_accelerator_work_proven") is True,
        layer,
        artifact,
        "native GPU proof did not prove meaningful accelerator work",
    )
    require(proof.get("embed_smoke_ok") is True, layer, artifact, "native embed smoke did not pass")
    require(proof.get("observation_source") == "native_log", layer, artifact, "GPU proof is not native-log-backed")
    identity = proof.get("runtime_identity")
    require(isinstance(identity, dict), layer, artifact, "GPU proof missing runtime identity")
    launch = identity.get("embedding_launch")
    require(isinstance(launch, dict), layer, artifact, "GPU proof missing native launch identity")
    require(launch.get("launch_mode") == "native_spawned", layer, artifact, "embedding launch is not native_spawned")
    pid = launch.get("pid")
    require(isinstance(pid, int) and pid > 0, layer, artifact, "native launch pid is invalid")
    require(
        isinstance(launch.get("spawned_at_epoch_ms"), int)
        and isinstance(launch.get("executable_path"), str)
        and isinstance(launch.get("log_path"), str)
        and isinstance(launch.get("launch_fingerprint_sha256"), str)
        and len(launch["launch_fingerprint_sha256"]) == 64,
        layer,
        artifact,
        "native launch identity is missing executable, start, log, or fingerprint evidence",
    )
    if expected_pid is not None:
        require(pid == expected_pid, layer, artifact, f"native launch pid changed: expected {expected_pid}, got {pid}")
    return launch


def require_agent_not_ready(payload: object, layer: str, artifact: Path) -> None:
    verdicts = payload.get("verdicts", []) if isinstance(payload, dict) else []
    agent = next((item for item in verdicts if item.get("goal") == "agent_packet_search"), None)
    require(agent is not None, layer, artifact, "missing agent_packet_search readiness verdict")
    require(agent.get("status") != "ready", layer, artifact, "dead native endpoint still reported agent ready")
    broker = payload.get("readiness_broker") if isinstance(payload, dict) else None
    proof = broker.get("gpu_proof") if isinstance(broker, dict) else None
    require(
        isinstance(proof, dict) and proof.get("proof_status") == "gpu_unverified",
        layer,
        artifact,
        "dead native endpoint did not invalidate GPU proof",
    )


def require_intel_default_backend_failure(payload: object, artifact: Path) -> None:
    require(isinstance(payload, dict), "intel_default_backend", artifact, "bootstrap output is not an object")
    state = payload.get("sidecar_state")
    status = payload.get("project_status")
    require(isinstance(state, dict), "intel_default_backend", artifact, "bootstrap output is missing sidecar_state")
    require(isinstance(status, dict), "intel_default_backend", artifact, "bootstrap output is missing project_status")
    require(payload.get("compose_started") is False, "intel_default_backend", artifact, "Intel default proof unexpectedly started Compose")
    require(payload.get("embed_reachable") is False, "intel_default_backend", artifact, "Intel default proof unexpectedly reached an embedding backend")
    require(status.get("retrieval_mode") != "full", "intel_default_backend", artifact, "Intel default proof incorrectly reported full retrieval")
    require(isinstance(status.get("repair"), dict), "intel_default_backend", artifact, "Intel default failure is missing actionable repair guidance")
    provider = state.get("embedding_accelerator_request_provider")
    require(provider != "metal", "intel_default_backend", artifact, "Intel default proof made a Metal claim")
    require(state.get("embedding_cpu_allowed") is False, "intel_default_backend", artifact, "Intel default proof silently allowed CPU retrieval")


def require_intel_cpu_external_ready(payload: object, artifact: Path, endpoint: str) -> None:
    require(isinstance(payload, dict), "intel_cpu_external", artifact, "bootstrap output is not an object")
    state = payload.get("sidecar_state")
    require(isinstance(state, dict), "intel_cpu_external", artifact, "bootstrap output is missing sidecar_state")
    require(payload.get("compose_started") is False, "intel_cpu_external", artifact, "external endpoint proof unexpectedly started Compose")
    require(payload.get("embed_reachable") is True, "intel_cpu_external", artifact, "explicit CPU/external embedding endpoint was not reachable")
    require(state.get("embed_url") == endpoint, "intel_cpu_external", artifact, "sidecar state did not retain the explicit embedding endpoint")
    require(state.get("embedding_device_policy") == "cpu_allowed", "intel_cpu_external", artifact, "CPU policy was not labelled cpu_allowed")
    require(state.get("embedding_device_state") == "cpu", "intel_cpu_external", artifact, "CPU runtime was not labelled cpu")
    require(state.get("embedding_device_observation_source") == "cpu_policy", "intel_cpu_external", artifact, "CPU runtime observation source is not cpu_policy")
    require(state.get("embedding_cpu_allowed") is True, "intel_cpu_external", artifact, "CPU runtime did not report explicit CPU allowance")
    require(state.get("embedding_accelerator_requested") is False, "intel_cpu_external", artifact, "CPU runtime still requested an accelerator")
    require(state.get("embedding_accelerator_request_provider") is None, "intel_cpu_external", artifact, "CPU runtime retained an accelerator provider")
    require("metal" not in json.dumps(payload).lower(), "intel_cpu_external", artifact, "Intel CPU/external proof made a Metal claim")


@contextlib.contextmanager
def embedding_probe_server():
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
            length = int(self.headers.get("content-length", "0"))
            if length:
                self.rfile.read(length)
            body = json.dumps({"data": [{"index": 0, "embedding": [0.0] * 768}]}).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, _format: str, *_args: object) -> None:
            return

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/v1/embeddings"
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)


def wait_for_process_exit(pid: int, timeout_secs: float = 15.0) -> None:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        try:
            waited_pid, _status = os.waitpid(pid, os.WNOHANG)
            if waited_pid == pid:
                return
        except ChildProcessError:
            pass
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.1)
    raise TimeoutError(f"native embedding pid {pid} did not exit after SIGTERM")


def darwin_process_argv(pid: int) -> tuple[str, list[str]] | None:
    if sys.platform != "darwin":
        return None
    libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    libc.sysctl.argtypes = [
        ctypes.POINTER(ctypes.c_int),
        ctypes.c_uint,
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_size_t),
        ctypes.c_void_p,
        ctypes.c_size_t,
    ]
    libc.sysctl.restype = ctypes.c_int
    mib = (ctypes.c_int * 3)(1, 49, pid)  # CTL_KERN, KERN_PROCARGS2, pid
    size = ctypes.c_size_t(0)
    if libc.sysctl(mib, 3, None, ctypes.byref(size), None, 0) != 0 or size.value < 8:
        return None
    buffer = ctypes.create_string_buffer(size.value)
    if libc.sysctl(mib, 3, buffer, ctypes.byref(size), None, 0) != 0:
        return None
    data = buffer.raw[: size.value]
    argc = struct.unpack_from("=i", data, 0)[0]
    if argc <= 0 or argc > 4096:
        return None
    cursor = 4

    def read_c_string(offset: int) -> tuple[str, int] | None:
        end = data.find(b"\0", offset)
        if end < 0:
            return None
        return data[offset:end].decode("utf-8", errors="surrogateescape"), end + 1

    executable_entry = read_c_string(cursor)
    if executable_entry is None:
        return None
    executable, cursor = executable_entry
    while cursor < len(data) and data[cursor] == 0:
        cursor += 1
    argv = []
    for _ in range(argc):
        entry = read_c_string(cursor)
        if entry is None:
            return None
        argument, cursor = entry
        argv.append(argument)
    return executable, argv


def registered_native_process_snapshot(launch: dict) -> dict:
    pid = launch.get("pid")
    if not isinstance(pid, int) or pid <= 0:
        return {"pid": pid, "status": "invalid_pid"}
    result = subprocess.run(
        ["ps", "-p", str(pid), "-o", "lstart="],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env={**os.environ, "LC_ALL": "C"},
        check=False,
    )
    if result.returncode != 0 or not result.stdout.strip():
        return {"pid": pid, "status": "already_exited", "stderr": result.stderr}
    start_text = result.stdout.strip()
    executable = launch.get("executable_path")
    arguments = launch.get("launch_args")
    exact_process = darwin_process_argv(pid)
    recorded_start = launch.get("spawned_at_epoch_ms")
    observed_start = None
    try:
        parsed_start = datetime.datetime.strptime(start_text, "%a %b %d %H:%M:%S %Y")
        observed_start = int(time.mktime(parsed_start.timetuple()) * 1000)
    except ValueError:
        pass
    start_matches = (
        isinstance(recorded_start, int)
        and observed_start is not None
        and abs(observed_start - recorded_start) <= 5_000
    )
    exact_executable = None
    exact_argv = None
    argv_matches = False
    if exact_process is not None and isinstance(executable, str) and isinstance(arguments, list):
        exact_executable, exact_argv = exact_process
        argv_matches = (
            os.path.realpath(exact_executable) == os.path.realpath(executable)
            and exact_argv[1:] == [str(item) for item in arguments]
        )
    matches = argv_matches and start_matches
    return {
        "pid": pid,
        "status": "matching" if matches else "identity_mismatch",
        "observed_executable": exact_executable,
        "observed_argv": exact_argv,
        "observed_start": start_text,
        "observed_start_epoch_ms": observed_start,
        "recorded_spawned_at_epoch_ms": recorded_start,
        "expected_executable": executable,
        "expected_arguments": arguments,
    }


def terminate_registered_native_process(launch: dict) -> dict:
    snapshot = registered_native_process_snapshot(launch)
    if snapshot["status"] in {"already_exited", "invalid_pid"}:
        return snapshot
    if snapshot["status"] != "matching":
        raise RuntimeError(
            f"refusing to terminate native pid {snapshot.get('pid')}: registered identity no longer matches"
        )
    pid = snapshot["pid"]
    os.kill(pid, signal.SIGTERM)
    try:
        wait_for_process_exit(pid)
        snapshot["status"] = "terminated"
    except TimeoutError:
        os.kill(pid, signal.SIGKILL)
        wait_for_process_exit(pid, timeout_secs=5)
        snapshot["status"] = "killed"
    return snapshot


def port_reachability(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.25)
        return probe.connect_ex(("127.0.0.1", port)) == 0


def cleanup_registered_proof_temp_root(args: argparse.Namespace) -> None:
    cleanup_run = getattr(args, "run", subprocess.run)
    candidate_root = Path(args.cleanup_proof_temp_root)
    if candidate_root.is_symlink() or not candidate_root.is_dir():
        raise RuntimeError(f"proof cleanup root is missing or unsafe: {candidate_root}")
    root = candidate_root.resolve(strict=True)
    runner_temp = os.environ.get("RUNNER_TEMP", "").strip()
    if runner_temp:
        canonical_runner_temp = Path(runner_temp).resolve(strict=True)
        if root.parent != canonical_runner_temp or not root.name.startswith("codestory-metal-proof-owned-"):
            raise RuntimeError(f"proof cleanup root is outside the exact runner temp boundary: {root}")
    marker = root / PROOF_TEMP_OWNER_FILE
    if marker.is_symlink() or not marker.is_file():
        raise RuntimeError(f"proof cleanup root has no safe ownership marker: {root}")
    ownership = read_json_file(marker)
    if not isinstance(ownership, dict) or ownership.get("owner") != "codestory-macos-metal-proof":
        raise RuntimeError(f"proof cleanup ownership marker is invalid: {marker}")
    repository = os.environ.get("GITHUB_REPOSITORY")
    if repository and ownership.get("repository") != repository:
        raise RuntimeError(
            f"proof cleanup marker repository does not match {repository!r}: {marker}"
        )
    archive_name = ownership.get("archive_name")
    archive_digest = ownership.get("archive_sha256")
    if (
        not isinstance(archive_name, str)
        or Path(archive_name).name != archive_name
        or not isinstance(archive_digest, str)
        or len(archive_digest) != 64
    ):
        raise RuntimeError(f"proof cleanup marker has no bound archive identity: {marker}")
    owned_archive = root / archive_name
    if owned_archive.is_symlink() or not owned_archive.is_file():
        raise RuntimeError(f"proof cleanup bound archive is missing or unsafe: {owned_archive}")
    if sha256_file(owned_archive) != archive_digest:
        raise RuntimeError(f"proof cleanup bound archive digest changed: {owned_archive}")
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    if out_dir.is_relative_to(root):
        raise RuntimeError("proof cleanup artifacts must be outside the removable proof root")
    project_value = ownership.get("project")
    project = (
        Path(project_value).resolve(strict=False)
        if isinstance(project_value, str)
        else Path(args.project).resolve(strict=False)
    )
    results = {
        "root": str(root),
        "cache_cleanup": [],
        "native_processes": [],
        "ports": [],
        "errors": [],
    }
    registered_sidecars = ownership.get("sidecars", [])
    if not registered_sidecars or not isinstance(registered_sidecars, list) or not all(
        isinstance(item, dict) for item in registered_sidecars
    ):
        raise RuntimeError(f"proof cleanup marker has invalid registered sidecars: {marker}")
    recorded_launches = [
        item for item in ownership.get("launches", []) if isinstance(item, dict)
    ]
    seen_pids = set()

    def terminate_launches(launches: list[dict]) -> None:
        for launch in launches:
            fingerprint = (launch.get("pid"), launch.get("launch_fingerprint_sha256"))
            if fingerprint in seen_pids:
                continue
            seen_pids.add(fingerprint)
            try:
                results["native_processes"].append(terminate_registered_native_process(launch))
            except Exception as exc:
                error = f"{type(exc).__name__}: {exc}"
                results["native_processes"].append(
                    {"pid": launch.get("pid"), "status": "failed", "error": error}
                )
                results["errors"].append(
                    {"kind": "native_process", "pid": launch.get("pid"), "error": error}
                )

    # Marker-bound process identities are independent of ephemeral project and
    # Compose files, so always attempt them before inspecting stale sidecar state.
    terminate_launches(recorded_launches)
    state_launches = []
    for identity in registered_sidecars:
        state_file = Path(str(identity.get("state_file", "")))
        if not state_file.is_file() or state_file.is_symlink():
            continue
        try:
            validated = validated_proof_compose_state(
                Path(str(identity["cache_root"])),
                Path(str(identity["project"])),
                read_json_file,
                run_id=str(identity["run_id"]),
                registered_identity=identity,
                allow_missing_compose_file=True,
            )
            if validated is not None:
                launch = validated[1].get("embedding_launch")
                if isinstance(launch, dict):
                    state_launches.append(launch)
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            results["errors"].append(
                {
                    "kind": "sidecar_state_validation",
                    "state_file": str(state_file),
                    "error": error,
                }
            )
    terminate_launches(state_launches)
    for index, value in enumerate(ownership.get("cache_roots", [])):
        if not isinstance(value, str):
            error = "proof cleanup marker contains a non-string cache root"
            results["cache_cleanup"].append(
                {"cache_root": repr(value), "status": "failed", "error": error}
            )
            results["errors"].append({"kind": "cache_cleanup", "error": error})
            continue
        cache = Path(value)
        canonical = cache.resolve(strict=False)
        if not canonical.is_relative_to(root) or not cache.name.startswith("codestory-packaged-"):
            error = f"refusing unregistered proof cache cleanup: {cache}"
            results["cache_cleanup"].append(
                {"cache_root": str(cache), "status": "failed", "error": error}
            )
            results["errors"].append(
                {"kind": "cache_cleanup", "cache_root": str(cache), "error": error}
            )
            continue
        if not cache.exists():
            results["cache_cleanup"].append({"cache_root": str(cache), "status": "already_removed"})
            continue
        artifact = out_dir / f"registered-cache-cleanup-{index}.json"
        try:
            preserve_native_embedding_evidence(cache, out_dir, f"registered-cache-{index}")
            cleanup_proof_cache(
                None,
                project,
                cache,
                artifact,
                run=cleanup_run,
                registered_sidecars=registered_sidecars,
                direct_only=True,
            )
            results["cache_cleanup"].append({"cache_root": str(cache), "status": "removed"})
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            results["cache_cleanup"].append(
                {"cache_root": str(cache), "status": "failed", "error": error}
            )
            results["errors"].append({"kind": "cache_cleanup", "cache_root": str(cache), "error": error})
    for port in ownership.get("ports", []):
        if isinstance(port, int) and 0 < port <= 65535:
            results["ports"].append({"port": port, "reachable_after_cleanup": port_reachability(port)})
    remaining_ports = [item["port"] for item in results["ports"] if item["reachable_after_cleanup"]]
    if remaining_ports:
        results["errors"].append(
            {"kind": "ports", "error": f"proof-owned ports remained reachable: {remaining_ports}"}
        )
    if not results["errors"]:
        remove_tree_with_retry(root)
        results["root_removed"] = not root.exists()
    else:
        results["root_removed"] = False
    write_json(out_dir / "proof-owned-cleanup.json", results)
    if results["errors"]:
        raise RuntimeError(
            "proof-owned cleanup had failures after attempting every resource: "
            + "; ".join(item["error"] for item in results["errors"])
        )


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
    empty_model_dir: Path,
    archive: Path,
) -> dict:
    require_plugin_manifest_version(plugin_root, expected_version)
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), "managed_plugin_convergence", artifact, f"plugin launcher is missing: {launcher}")
    with temporary_directory_with_retry("codestory-plugin-data-", artifact.parent) as data:
        plugin_data = Path(data)
        prior_version = "0.0.0"
        archive_suffix = archive.name.removeprefix(f"codestory-cli-v{expected_version}-")
        extension = ".zip" if archive_suffix.endswith(".zip") else ".tar.gz"
        target = archive_suffix.removesuffix(extension)
        require(
            target and target != archive_suffix,
            "managed_plugin_convergence",
            artifact,
            f"could not derive release target from {archive.name}",
        )
        prior_dir = plugin_data / "codestory-cli" / prior_version
        prior_bin = prior_dir / "bin" / ("codestory-cli.cmd" if os.name == "nt" else "codestory-cli")
        prior_bin.parent.mkdir(parents=True)
        if os.name == "nt":
            prior_bin.write_text(
                f"@echo off\r\nif \"%1\"==\"--version\" (echo codestory-cli {prior_version}& exit /b 0)\r\nexit /b 90\r\n",
                encoding="utf-8",
            )
        else:
            prior_bin.write_text(
                f"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codestory-cli {prior_version}'; exit 0; fi\nexit 90\n",
                encoding="utf-8",
            )
            prior_bin.chmod(prior_bin.stat().st_mode | stat.S_IXUSR)
        prior_archive = f"codestory-cli-v{prior_version}-{target}{extension}"
        write_json(
            prior_dir / "manifest.json",
            {
                "path": prior_bin.relative_to(prior_dir).as_posix(),
                "sha256": sha256_file(prior_bin),
                "version": prior_version,
                "build_source": "github_release",
                "repo_ref": f"v{prior_version}",
                "archive": prior_archive,
                "archive_url": (release_dir / prior_archive).resolve().as_uri(),
                "archive_sha256": "0" * 64,
                "target": target,
                "provisioned_at": "1970-01-01T00:00:00.000Z",
                "stdio_initialize_verified": True,
            },
        )
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
            fail("managed_plugin_convergence", policy_artifact, f"plugin sidecar policy enable failed: {exc}")
        write_json(
            policy_artifact,
            {"returncode": policy.returncode, "stdout": policy.stdout, "stderr": policy.stderr},
        )
        require(
            policy.returncode == 0
            and policy_path.is_file()
            and read_json_file(policy_path).get("state") == "enabled",
            "managed_plugin_convergence",
            policy_artifact,
            "plugin sidecar policy enable failed",
        )
        plugin_env = proof_environment({
            **os.environ,
            "CODESTORY_CLI": "",
            "CODESTORY_CACHE_ROOT": str(cache_root),
            "CODESTORY_EMBED_MODEL_DIR": str(empty_model_dir),
            "CODESTORY_PLUGIN_RELEASE_DIR": str(release_dir),
            "CODESTORY_PLUGIN_SIDECAR_POLICY_PATH": str(policy_path),
            "PLUGIN_DATA": str(plugin_data),
        })
        ownership_before_status = proof_ownership_snapshot(cache_root)
        observational_artifact = artifact.with_name("managed-convergence-status-observational.json")
        observational_status = stdio_status_command(
            ["node", str(launcher)],
            observational_artifact,
            timeout_secs,
            project,
            layer="managed_plugin_convergence",
            cwd=project,
            env=plugin_env,
        )
        ownership_after_status = proof_ownership_snapshot(cache_root)
        require(
            ownership_after_status == ownership_before_status,
            "managed_plugin_convergence",
            observational_artifact,
            "observational status changed local-refresh or ready-repair ownership files",
        )
        require_managed_observational_status(
            observational_status,
            observational_artifact,
            expected_version,
            archive_cli,
        )
        status = stdio_status_command(
            ["node", str(launcher)],
            artifact,
            timeout_secs,
            project,
            layer="managed_plugin_convergence",
            cwd=project,
            env=plugin_env,
            extra_requests=[
                {
                    "jsonrpc": "2.0",
                    "id": "ground_activation",
                    "method": "tools/call",
                    "params": {
                        "name": "ground",
                        "arguments": {"project": str(project), "budget": "strict"},
                    },
                },
                *[
                    {
                        "jsonrpc": "2.0",
                        "id": f"setup_poll_{index}",
                        "method": "tools/call",
                        "params": {
                            "name": "sidecar_setup",
                            "arguments": {"project": str(project), "action": "status"},
                        },
                        "_delay_before_secs": 2,
                    }
                    for index in range(1, 6)
                ],
                {
                    "jsonrpc": "2.0",
                    "id": "status_after_convergence",
                    "method": "resources/read",
                    "params": {"uri": STATUS_URI, "project": str(project)},
                },
            ],
            cleanup_status_workers=True,
        )
        require_managed_plugin_convergence(status, artifact, expected_version, archive_cli)
        retention = status.get("plugin_runtime", {}).get("managed_cli_retention", {})
        retained = retention.get("retained", []) if isinstance(retention, dict) else []
        require(
            retention.get("active_version") == expected_version
            and any(item.get("version") == prior_version and item.get("reason") == "rollback" for item in retained),
            "managed_plugin_convergence",
            artifact,
            "managed upgrade did not retain the verified prior version as rollback",
        )
        write_json(
            artifact.with_name("managed-plugin-upgrade.json"),
            {
                "prior_version": prior_version,
                "requested_version": expected_version,
                "server_version": status.get("server_version"),
                "server_executable": status.get("server_executable"),
                "retention": retention,
            },
        )
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

        for request_spec in requests:
            request = dict(request_spec)
            delay_before_secs = request.pop("_delay_before_secs", 0)
            if delay_before_secs:
                time.sleep(delay_before_secs)
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


def require_managed_binary_provenance(
    status: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> None:
    require_plugin_provenance(status, artifact, expected_version, "managed_plugin_convergence")
    plugin_runtime = status["plugin_runtime"]
    managed_binary = Path(plugin_runtime.get("managed_binary_path", ""))
    server_executable = Path(status.get("server_executable", ""))
    require(
        managed_binary.is_file()
        and server_executable.is_file()
        and managed_binary.samefile(server_executable),
        "managed_plugin_convergence",
        artifact,
        "managed_binary_path does not identify the executable serving MCP",
    )
    archive_sha256 = sha256_file(archive_cli)
    server_sha256 = sha256_file(server_executable)
    require(
        archive_sha256 == server_sha256 == status.get("server_executable_sha256"),
        "managed_plugin_convergence",
        artifact,
        "managed MCP executable does not match the packaged archive binary",
    )


def local_freshness_status(status: dict) -> str | None:
    for name in ("effective_index_freshness", "index_freshness"):
        value = status.get(name, {}).get("status")
        if isinstance(value, str):
            return value
    return None


def structured_content_from_response(response: dict) -> dict | None:
    content = response.get("result", {}).get("structuredContent")
    return content if isinstance(content, dict) else None


def require_managed_observational_status(
    status: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> None:
    require_managed_binary_provenance(status, artifact, expected_version, archive_cli)
    require(
        local_freshness_status(status) == "stale",
        "managed_plugin_convergence",
        artifact,
        f"status did not observe the seeded project as stale: {local_freshness_status(status)!r}",
    )
    require(
        isinstance(status.get("index_publication", {}).get("generation"), int),
        "managed_plugin_convergence",
        artifact,
        "stale status did not retain a complete published generation",
    )
    require(
        not status.get("readiness_broker", {}).get("operations"),
        "managed_plugin_convergence",
        artifact,
        "observational status reported a running readiness operation",
    )
    setup = status.get("sidecar_setup", {})
    require(
        setup.get("active_repair") is None
        and setup.get("last_worker_result") is None
        and status.get("status_resource_auto_repair") is None,
        "managed_plugin_convergence",
        artifact,
        "observational status started or reported a repair attempt",
    )
    require(
        status.get("allowed_surfaces", {}).get("packet", {}).get("allowed") is False
        and status.get("allowed_surfaces", {}).get("search", {}).get("allowed") is False,
        "managed_plugin_convergence",
        artifact,
        "missing sidecars did not keep packet/search fail closed",
    )


def require_managed_plugin_convergence(
    status_before: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> None:
    require_managed_observational_status(status_before, artifact, expected_version, archive_cli)
    initial_generation = status_before["index_publication"]["generation"]
    ground = transcript_response(artifact, "ground_activation") or {}
    require(
        structured_content_from_response(ground) is not None
        and ground.get("result", {}).get("isError") is not True,
        "managed_plugin_convergence",
        artifact,
        "ground activation did not serve a complete publication",
    )
    setup_snapshots = []
    for request_id in (
        "setup_poll_1",
        "setup_poll_2",
        "setup_poll_3",
        "setup_poll_4",
        "setup_poll_5",
    ):
        setup = structured_content_from_response(transcript_response(artifact, request_id) or {})
        require(
            isinstance(setup, dict),
            "managed_plugin_convergence",
            artifact,
            f"{request_id} did not return sidecar setup status",
        )
        setup_snapshots.append(setup)
    attempt_records = [
        record
        for setup in setup_snapshots
        for record in (setup.get("active_repair"), setup.get("last_worker_result"))
        if isinstance(record, dict)
        and isinstance(record.get("attempt_id"), str)
        and record.get("attempt_id")
    ]
    attempt_ids = {record["attempt_id"] for record in attempt_records}
    require(
        len(attempt_ids) == 1,
        "managed_plugin_convergence",
        artifact,
        f"ground activation did not retain exactly one repair attempt: {sorted(attempt_ids)}",
    )
    expected_project = status_before.get("project_root")
    namespaces = {record.get("namespace") for record in attempt_records}
    require(
        isinstance(expected_project, str)
        and expected_project
        and len(namespaces) == 1
        and all(isinstance(namespace, str) and namespace for namespace in namespaces)
        and all(
            record.get("project_root") == expected_project
            and record.get("profile") == "agent"
            and record.get("run_id") == "shared-agent"
            for record in attempt_records
        ),
        "managed_plugin_convergence",
        artifact,
        "ground activation repair records did not preserve one durable project/profile/run/namespace identity",
    )
    terminal = next(
        (
            setup.get("last_worker_result")
            for setup in reversed(setup_snapshots)
            if isinstance(setup.get("last_worker_result"), dict)
            and setup.get("active_repair") is None
        ),
        None,
    )
    require(
        isinstance(terminal, dict)
        and terminal.get("attempt_id") in attempt_ids
        and terminal.get("run_id") == "shared-agent"
        and terminal.get("outcome") in {"failed", "succeeded"},
        "managed_plugin_convergence",
        artifact,
        "automatic shared-agent repair did not reach a durable terminal result",
    )
    status_after = status_from_resource_response(
        transcript_response(artifact, "status_after_convergence") or {}
    )
    require(
        isinstance(status_after, dict),
        "managed_plugin_convergence",
        artifact,
        "status after ground activation was unavailable",
    )
    require_managed_binary_provenance(status_after, artifact, expected_version, archive_cli)
    require(
        local_freshness_status(status_after) == "fresh"
        and isinstance(status_after.get("index_publication", {}).get("generation"), int)
        and status_after["index_publication"]["generation"] > initial_generation,
        "managed_plugin_convergence",
        artifact,
        "ground activation did not publish a newer complete local generation",
    )
    gpu_proof = status_after.get("readiness_broker", {}).get("gpu_proof", {})
    require(
        status_after.get("allowed_surfaces", {}).get("packet", {}).get("allowed") is False
        and status_after.get("allowed_surfaces", {}).get("search", {}).get("allowed") is False
        and (
            gpu_proof.get("proof_status") != "verified"
            or gpu_proof.get("meaningful_accelerator_work_proven") is not True
            or gpu_proof.get("embed_smoke_ok") is not True
        ),
        "managed_plugin_convergence",
        artifact,
        "accelerator-required packet/search opened without verified runtime smoke proof",
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
        source_project = project
        if getattr(args, "native_edge_cases", False):
            edge_project = Path(temp) / "CodeStory project with spaces ü"
            shutil.copytree(
                source_project,
                edge_project,
                symlinks=True,
                ignore=shutil.ignore_patterns(".git", "target"),
            )
            project = edge_project.resolve(strict=True)
        unpacked = Path(temp) / "unpacked"
        unpacked.mkdir()
        unpack_archive(archive, unpacked)
        cli = find_cli(unpacked)
        plugin_skill = find_plugin_skill(unpacked)
        cache_root = Path(tempfile.mkdtemp(prefix="codestory-packaged-proof-cache-")).resolve(strict=True)
        proof_cleanup_artifact = out_dir / "proof-cache-cleanup.json"
        proof_cleanup_sidecars = proof_agent_identities([cache_root], project)
        cleanup_stack.push(
            cleanup_proof_cache_on_exit(
                cli,
                project,
                cache_root,
                proof_cleanup_artifact,
                registered_sidecars=proof_cleanup_sidecars,
            )
        )
        proof_env = proof_environment({**os.environ, "CODESTORY_CACHE_ROOT": str(cache_root)})
        stdio_cache_root = Path(tempfile.mkdtemp(prefix="codestory-packaged-stdio-cache-")).resolve(strict=True)
        stdio_cleanup_artifact = out_dir / "stdio-cache-cleanup.json"
        stdio_cleanup_sidecars = proof_agent_identities([stdio_cache_root], project)
        cleanup_stack.push(
            cleanup_proof_cache_on_exit(
                cli,
                project,
                stdio_cache_root,
                stdio_cleanup_artifact,
                registered_sidecars=stdio_cleanup_sidecars,
            )
        )
        stdio_env = {**os.environ, "CODESTORY_CACHE_ROOT": str(stdio_cache_root)}
        register_proof_temp_ownership(project, [cache_root, stdio_cache_root], archive)

        corrupt_server_seed = None
        if getattr(args, "native_edge_cases", False):
            corrupt_server_seed = seed_corrupt_managed_server(cache_root, project)

        summary = {
            "archive": str(archive),
            "cli": str(cli),
            "plugin_skill": str(plugin_skill),
            "project": str(project),
            "source_project": str(source_project),
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
        if args.version_only or args.managed_plugin_handoff or getattr(args, "intel_runtime_policy", False):
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

        if getattr(args, "intel_runtime_policy", False):
            machine = os.uname().machine.lower() if hasattr(os, "uname") else ""
            require(
                sys.platform == "darwin" and machine == "x86_64",
                "intel_runtime_host",
                archive,
                f"Intel runtime policy proof requires macOS x86_64, got {sys.platform}/{machine}",
            )
            default_env = proof_agent_environment(proof_env, PROOF_LOCAL_RUN_ID)
            for name in (
                "CODESTORY_EMBED_ALLOW_CPU",
                "CODESTORY_EMBED_DEVICE_POLICY",
                "CODESTORY_EMBED_DEVICE_PROVIDER",
                "CODESTORY_EMBED_DEVICE_STATE",
                "CODESTORY_EMBED_LLAMACPP_URL",
                "CODESTORY_EMBED_SERVER_LAUNCH",
            ):
                default_env.pop(name, None)
            default_env["CODESTORY_EMBED_BACKEND"] = "llamacpp"
            default_artifact = out_dir / "intel-default-backend.json"
            default_payload = run_command(
                cli,
                "intel_default_backend",
                [
                    "retrieval",
                    "bootstrap",
                    "--project",
                    str(project),
                    "--profile",
                    "agent",
                    "--run-id",
                    PROOF_LOCAL_RUN_ID,
                    "--skip-compose",
                    "--wait-secs",
                    "0",
                    "--format",
                    "json",
                    "--output-file",
                    str(default_artifact),
                ],
                default_artifact,
                args.timeout_secs,
                env=default_env,
            )
            require_intel_default_backend_failure(default_payload, default_artifact)
            summary["artifacts"]["intel_default_backend"] = str(default_artifact)

            with embedding_probe_server() as endpoint:
                cpu_env = {
                    **default_env,
                    "CODESTORY_EMBED_ALLOW_CPU": "1",
                    "CODESTORY_EMBED_LLAMACPP_URL": endpoint,
                    "CODESTORY_EMBED_SERVER_LAUNCH": "external_endpoint",
                }
                cpu_artifact = out_dir / "intel-cpu-external.json"
                cpu_payload = run_command(
                    cli,
                    "intel_cpu_external",
                    [
                        "retrieval",
                        "bootstrap",
                        "--project",
                        str(project),
                        "--profile",
                        "agent",
                        "--run-id",
                        PROOF_LOCAL_RUN_ID,
                        "--skip-compose",
                        "--wait-secs",
                        "0",
                        "--format",
                        "json",
                        "--output-file",
                        str(cpu_artifact),
                    ],
                    cpu_artifact,
                    args.timeout_secs,
                    env=cpu_env,
                )
                require_intel_cpu_external_ready(cpu_payload, cpu_artifact, endpoint)
                summary["artifacts"]["intel_cpu_external"] = str(cpu_artifact)
            write_json(out_dir / "summary.json", summary)
            return

        if args.managed_plugin_handoff:
            convergence_project = Path(temp) / "managed-convergence-project"
            empty_model_dir = Path(temp) / "managed-convergence-empty-model"
            write_managed_convergence_fixture(convergence_project)
            proof_cleanup_sidecars.extend(
                proof_agent_identities([cache_root], convergence_project)
            )
            empty_model_dir.mkdir()
            summary["managed_convergence_project"] = str(convergence_project)
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
                    str(convergence_project),
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
                    str(convergence_project),
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
            make_managed_convergence_fixture_stale(convergence_project)

            plugin_artifact = out_dir / "managed-plugin-convergence.json"
            plugin_status = plugin_stdio_handoff(
                Path(args.plugin_root).resolve(),
                archive.parent,
                convergence_project,
                cache_root,
                plugin_artifact,
                args.timeout_secs,
                args.expected_version,
                cli,
                empty_model_dir,
                archive,
            )
            register_current_proof_runtime(cache_root)
            summary["artifacts"]["managed_plugin_convergence"] = str(plugin_artifact)
            summary["artifacts"]["managed_plugin_upgrade"] = str(
                plugin_artifact.with_name("managed-plugin-upgrade.json")
            )
            write_json(out_dir / "summary.json", summary)
            return

        local_env = proof_agent_environment(proof_env, PROOF_LOCAL_RUN_ID)
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
            env=local_env,
        )
        require_agent_ready(ready, "ready", ready_artifact)
        register_current_proof_runtime(cache_root)
        summary["artifacts"]["ready"] = str(ready_artifact)
        if corrupt_server_seed is not None:
            corrupt_server_artifact = out_dir / "native-corrupt-server-repair.json"
            require_managed_server_repaired(corrupt_server_seed, corrupt_server_artifact)
            summary["artifacts"]["native_corrupt_server_repair"] = str(corrupt_server_artifact)
        write_json(out_dir / "summary.json", summary)

        if getattr(args, "native_accelerator_lifecycle", False):
            original_launch = require_native_accelerator_ready(ready, "native_ready", ready_artifact)
            original_pid = original_launch["pid"]
            register_proof_launch(original_launch)

            survival_artifact = out_dir / "native-runtime-survival.json"
            survival = run_command(
                cli,
                "native_runtime_survival",
                [
                    "ready",
                    "--goal",
                    "agent",
                    "--project",
                    str(project),
                    "--format",
                    "json",
                    "--output-file",
                    str(survival_artifact),
                ],
                survival_artifact,
                args.timeout_secs,
                env=local_env,
            )
            survival_launch = require_native_accelerator_ready(
                survival,
                "native_runtime_survival",
                survival_artifact,
                expected_pid=original_pid,
            )
            register_proof_launch(survival_launch)
            summary["artifacts"]["native_runtime_survival"] = str(survival_artifact)
            cold_warm_evidence = preserve_native_embedding_evidence(
                cache_root,
                out_dir,
                "native-cold-warm",
                required=True,
                exact_launch=survival_launch,
            )
            summary["artifacts"]["native_cold_warm_launch"] = str(cold_warm_evidence)
            write_json(out_dir / "summary.json", summary)

            os.kill(original_pid, signal.SIGTERM)
            try:
                wait_for_process_exit(original_pid)
            except TimeoutError as exc:
                fail("native_runtime_shutdown", survival_artifact, str(exc))

            blocked_artifact = out_dir / "native-runtime-dead-status.json"
            blocked = run_command(
                cli,
                "native_runtime_dead_status",
                [
                    "ready",
                    "--goal",
                    "agent",
                    "--project",
                    str(project),
                    "--format",
                    "json",
                    "--output-file",
                    str(blocked_artifact),
                ],
                blocked_artifact,
                args.timeout_secs,
                env=local_env,
            )
            require_agent_not_ready(blocked, "native_runtime_dead_status", blocked_artifact)
            summary["artifacts"]["native_runtime_dead_status"] = str(blocked_artifact)

            recovery_artifact = out_dir / "native-runtime-recovery.json"
            recovery = run_command(
                cli,
                "native_runtime_recovery",
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
                    str(recovery_artifact),
                ],
                recovery_artifact,
                args.timeout_secs,
                env=local_env,
            )
            recovered_launch = require_native_accelerator_ready(
                recovery,
                "native_runtime_recovery",
                recovery_artifact,
            )
            recovered_pid = recovered_launch["pid"]
            register_proof_launch(recovered_launch)
            require(
                recovered_pid != original_pid,
                "native_runtime_recovery",
                recovery_artifact,
                "repair reused the terminated native embedding pid",
            )
            summary["artifacts"]["native_runtime_recovery"] = str(recovery_artifact)
            recovery_evidence = preserve_native_embedding_evidence(
                cache_root,
                out_dir,
                "native-recovery",
                required=True,
                exact_launch=recovered_launch,
            )
            summary["artifacts"]["native_recovery_launch"] = str(recovery_evidence)
            write_json(out_dir / "summary.json", summary)

        doctor_artifact = out_dir / "doctor.json"
        doctor = run_command(
            cli,
            "doctor",
            ["doctor", "--project", str(project), "--format", "json", "--output-file", str(doctor_artifact)],
            doctor_artifact,
            args.timeout_secs,
            env=local_env,
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
            env=local_env,
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
            env=local_env,
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
            env=local_env,
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
            env=local_env,
        )
        require_packet_ready(packet, packet_artifact)
        summary["artifacts"]["packet"] = str(packet_artifact)
        write_json(out_dir / "summary.json", summary)

        stdio_status_payload = stdio_status(cli, project, stdio_artifact, args.timeout_secs, local_env)
        require_stdio_shape(stdio_status_payload, stdio_artifact, args.expected_version)
        allowed = stdio_status_payload.get("allowed_surfaces", {})
        if not all(allowed.get(name, {}).get("allowed") is True for name in ("packet", "search", "context")):
            shutil.copy2(stdio_artifact, out_dir / "serve-stdio-status-initial.json")
            stdio_status_payload = stdio_status(cli, project, stdio_artifact, args.timeout_secs, local_env)
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
                ground_count = 0
                managed_mode = os.environ.get("CODESTORY_FAKE_PLUGIN_MANAGED") == "1"
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
                        result = {"tools": [{"name": "ground"}, {"name": "packet"}, {"name": "search"}, {"name": "context"}, {"name": "sidecar_setup"}]}
                    elif request.get("method") == "resources/list":
                        resources = [{"uri": "codestory://status", "name": "CodeStory runtime status"}]
                        if fail != "resources_hidden":
                            resources.append({"uri": "codestory://agent-guide", "name": "CodeStory agent guide"})
                        result = {"resources": resources}
                    elif request.get("method") == "tools/call" and request.get("params", {}).get("name") == "ground":
                        repair_attempt = repair_attempt or "fake-activation-attempt"
                        ground_count += 1
                        ground = {
                            "root": request.get("params", {}).get("arguments", {}).get("project"),
                            "stats": {"file_count": 2, "node_count": 2},
                        }
                        result = {"content": [{"type": "text", "text": json.dumps(ground)}], "structuredContent": ground}
                    elif request.get("method") == "tools/call" and request.get("params", {}).get("name") == "sidecar_setup":
                        if request.get("params", {}).get("arguments", {}).get("action") == "status":
                            setup = {
                                "state": "enabled",
                                "active_repair": None,
                                "last_worker_result": ({
                                    "attempt_id": repair_attempt,
                                    "project_root": request.get("params", {}).get("arguments", {}).get("project"),
                                    "profile": "agent",
                                    "run_id": "shared-agent",
                                    "namespace": "fake-agent-namespace",
                                    "outcome": "failed",
                                    "exit_code": 1,
                                } if repair_attempt else None),
                                "activation_triggered_repair": bool(repair_attempt),
                            }
                            result = {"content": [{"type": "text", "text": json.dumps(setup)}], "structuredContent": setup}
                            response_id = request.get("id")
                            print(json.dumps({"jsonrpc": "2.0", "id": response_id, "result": result}), flush=True)
                            continue
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
                            "project_root": request.get("params", {}).get("project", os.getcwd()),
                            "effective_index_freshness": {"status": "fresh" if ground_count or not managed_mode else "stale"},
                            "index_freshness": {"status": "fresh" if ground_count or not managed_mode else "stale"},
                            "index_publication": {"generation": 2 if ground_count else 1},
                            "readiness_broker": {
                                "operations": ([{
                                    "operation_kind": "local_graph_refresh",
                                    "pid": refresh_worker_pid,
                                    "status": "running",
                                }] if refresh_worker_pid else []),
                                "gpu_proof": {
                                    "proof_status": "gpu_unverified",
                                    "meaningful_accelerator_work_proven": False,
                                    "embed_smoke_ok": False,
                                },
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
                                "managed_cli_retention": {
                                    "active_version": "9.9.9",
                                    "retained": [
                                        {"version": "9.9.9", "reason": "active"},
                                        {"version": "0.0.0", "reason": "rollback"},
                                    ],
                                },
                            },
                            "sidecar_setup": {
                                "state": "enabled",
                                "active_repair": None,
                                "last_worker_result": ({
                                    "attempt_id": repair_attempt,
                                    "project_root": request.get("params", {}).get("project", os.getcwd()),
                                    "profile": "agent",
                                    "run_id": "shared-agent",
                                    "namespace": "fake-agent-namespace",
                                    "outcome": "failed",
                                    "exit_code": 1,
                                } if repair_attempt else None),
                                "activation_triggered_repair": bool(repair_attempt),
                            },
                            "status_resource_auto_repair": None,
                            "readiness_lanes": {
                                "agent_packet_search": {"run_id": "shared-agent", "status": "blocked"},
                            },
                            "allowed_surfaces": {
                                "ground": {"allowed": True},
                                "packet": {"allowed": False if managed_mode else serve_allowed},
                                "search": {"allowed": False if managed_mode else serve_allowed},
                                "context": {"allowed": False if managed_mode else serve_allowed},
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


def expect_fake_gate_failure(
    args: argparse.Namespace,
    fail_layer: str,
    expected_layer: str,
    artifact_fragment: str,
    failure_message: str,
) -> None:
    os.environ["CODESTORY_FAKE_FAIL_LAYER"] = fail_layer
    try:
        try:
            run_gate(args)
        except GateFailure as exc:
            assert exc.layer == expected_layer
            assert artifact_fragment in str(exc.artifact)
        else:
            raise AssertionError(failure_message)
    finally:
        os.environ.pop("CODESTORY_FAKE_FAIL_LAYER", None)


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-self-test-") as temp:
        root = Path(temp)
        runner_temp = os.environ.get("RUNNER_TEMP", "").strip()
        proof_root_parent = Path(runner_temp).resolve(strict=True) if runner_temp else root

        def proof_owned_test_root(suffix: str) -> Path:
            path = proof_root_parent / f"codestory-metal-proof-owned-{suffix}"
            path.mkdir()
            return path

        evidence_cache = root / "native-evidence-cache"
        evidence_cache.mkdir()
        native_log = evidence_cache / "llama-server-native.log"
        native_log.write_text("launch marker\n" + "x" * 256 + "\noffloaded 13/13 layers to GPU\n", encoding="utf-8")
        write_json(
            evidence_cache / "retrieval-sidecars.json",
            {
                "owner": "codestory",
                "embedding_launch": {
                    "launch_mode": "native_spawned",
                    "pid": 1234,
                    "log_path": str(native_log),
                },
            },
        )
        evidence_artifact = preserve_native_embedding_evidence(
            evidence_cache,
            root,
            "native-evidence-self-test",
            required=True,
        )
        assert evidence_artifact is not None and evidence_artifact.is_file()
        assert (root / "native-evidence-self-test-llama-server-native.log").is_file()
        bounded_artifact = root / "bounded-native.log"
        bounded = bounded_file_copy(native_log, bounded_artifact, max_bytes=64)
        assert bounded["truncated"] is True
        bounded_body = bounded_artifact.read_text(encoding="utf-8")
        assert "launch marker" in bounded_body and "offloaded 13/13" in bounded_body

        registration_payload = {"launches": [], "ports": []}
        registered_launch = {
            "launch_mode": "native_spawned",
            "pid": 1234,
            "launch_fingerprint_sha256": "1" * 64,
        }
        record_proof_runtime_identity(
            registration_payload,
            registered_launch,
            [18080, 18080, 0, 65536, "18081"],
        )
        record_proof_runtime_identity(registration_payload, registered_launch, [18080])
        assert registration_payload == {
            "launches": [registered_launch],
            "ports": [18080],
        }

        if sys.platform == "darwin":
            probe_code = "import time; time.sleep(60)"
            process_probe = subprocess.Popen(
                [sys.executable, "-c", probe_code, "--exact-proof-child"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                launch = {
                    "pid": process_probe.pid,
                    "spawned_at_epoch_ms": int(time.time() * 1000),
                    "executable_path": sys.executable,
                    "launch_args": ["-c", probe_code, "--exact-proof-child"],
                }
                assert registered_native_process_snapshot(launch)["status"] == "matching"
                prefix_collision = {**launch, "launch_args": ["-c", probe_code, "--exact-proof"]}
                assert registered_native_process_snapshot(prefix_collision)["status"] == "identity_mismatch"
            finally:
                process_probe.terminate()
                process_probe.wait(timeout=5)

        stage = root / "pkg" / "codestory-cli-v9.9.9-test"
        stage.mkdir(parents=True)
        write_fake_cli(stage)
        fake_cli = find_cli(stage)
        compose_file = root / "docker" / "retrieval-compose.yml"
        compose_file.parent.mkdir()
        compose_file.write_text("services: {}\n", encoding="utf-8")

        def write_proof_compose_state(
            cache_root: Path,
            overrides: dict | None = None,
            *,
            state_project: Path = root,
        ) -> Path:
            cache_root.mkdir()
            identity = proof_agent_identity(cache_root, state_project, PROOF_LOCAL_RUN_ID)
            state_root = Path(identity["state_file"]).parent
            (state_root / "qdrant").mkdir(parents=True)
            (state_root / "lexical").mkdir()
            (state_root / "scip").mkdir()
            state = {
                "owner": "codestory",
                "profile": "agent",
                "run_id": PROOF_LOCAL_RUN_ID,
                "namespace": identity["namespace"],
                "compose_project": identity["compose_project"],
                "compose_file": str(compose_file),
                "qdrant_data_dir": identity["qdrant_data_dir"],
                "lexical_data_dir": identity["lexical_data_dir"],
                "scip_artifacts_root": identity["scip_artifacts_root"],
            }
            state.update(overrides or {})
            state_file = Path(identity["state_file"])
            write_json(state_file, state)
            return state_file

        compose_cleanup_root = root / "compose-cleanup"
        write_proof_compose_state(compose_cleanup_root)
        compose_cleanup_artifact = root / "compose-cleanup.json"
        compose_calls = []

        def successful_compose_cleanup(command, **kwargs):
            if command[0] == "docker":
                compose_calls.append((command, kwargs["env"]))
                namespace = kwargs["env"]["CODESTORY_SIDECAR_NAMESPACE"]
                qdrant_root = kwargs["env"]["CODESTORY_QDRANT_DATA_DIR"]
                container_id = f"container-{fnv1a_hex(qdrant_root)}"
                network_id = f"network-{fnv1a_hex(qdrant_root)}"
                created = "2026-07-12T12:00:00Z"
                if command[1:3] == ["container", "ls"]:
                    stdout = json.dumps({"ID": container_id})
                elif command[1:3] == ["network", "ls"]:
                    stdout = json.dumps({"ID": network_id})
                elif command[1:3] == ["container", "inspect"]:
                    stdout = json.dumps(
                        [
                            {
                                "Id": container_id,
                                "Name": f"/{namespace}-qdrant",
                                "Created": created,
                                "Config": {
                                    "Labels": {
                                        "com.docker.compose.project": namespace,
                                        "com.docker.compose.service": "qdrant",
                                        "dev.codestory.owner": "codestory",
                                        "dev.codestory.profile": "agent",
                                        "dev.codestory.namespace": namespace,
                                    }
                                },
                                "Mounts": [
                                    {
                                        "Type": "bind",
                                        "Source": qdrant_root,
                                        "Destination": "/qdrant/storage",
                                    }
                                ],
                            }
                        ]
                    )
                elif command[1:3] == ["network", "inspect"]:
                    stdout = json.dumps(
                        [
                            {
                                "Id": network_id,
                                "Name": f"{namespace}_default",
                                "Created": created,
                                "Labels": {
                                    "com.docker.compose.project": namespace,
                                    "com.docker.compose.network": "default",
                                },
                                "Containers": {container_id: {"Name": f"{namespace}-qdrant"}},
                            }
                        ]
                    )
                else:
                    stdout = "removed"
                return subprocess.CompletedProcess(command, 0, stdout, "")
            return subprocess.run(command, **kwargs)

        compose_state = read_json_file(
            Path(proof_agent_identity(compose_cleanup_root, root, PROOF_LOCAL_RUN_ID)["state_file"])
        )
        registered_resources = docker_compose_resource_snapshot(
            compose_state,
            successful_compose_cleanup,
        )
        remaining_registered_resources = json.loads(json.dumps(registered_resources))
        remaining_registered_resources["containers"] = []
        remaining_registered_resources["networks"][0]["attached_container_ids"] = []
        validate_proof_docker_resources(
            compose_state,
            remaining_registered_resources,
            registered_resources,
        )
        foreign_remaining_resources = json.loads(json.dumps(remaining_registered_resources))
        foreign_remaining_resources["networks"][0]["id"] = "unregistered-network"
        try:
            validate_proof_docker_resources(
                compose_state,
                foreign_remaining_resources,
                registered_resources,
            )
        except RuntimeError as exc:
            assert "absent from proof registration" in str(exc)
        else:
            raise AssertionError("partial cleanup retry must reject unregistered Docker IDs")

        cleanup_proof_cache(
            fake_cli,
            root,
            compose_cleanup_root,
            compose_cleanup_artifact,
            successful_compose_cleanup,
        )
        assert not compose_cleanup_root.exists()
        removal_calls = [command for command, _env in compose_calls if "rm" in command]
        assert removal_calls[0][1:4] == ["container", "rm", "-f"]
        assert removal_calls[1][1:3] == ["network", "rm"]
        assert compose_calls[0][1]["CODESTORY_SIDECAR_NAMESPACE"].startswith("codestory-agent-")
        assert compose_calls[0][1]["CODESTORY_SIDECAR_NAMESPACE"] != "codestory"
        compose_cleanup = read_json_file(compose_cleanup_artifact)
        assert compose_cleanup["removed"] is True
        assert any(
            item.get("kind") == "docker_container_remove"
            for item in compose_cleanup["commands"]
        )

        foreign_collision_root = root / "compose-cleanup-foreign-collision"
        write_proof_compose_state(foreign_collision_root)
        foreign_mount = root / "foreign-qdrant-cache"
        foreign_mount.mkdir()
        foreign_collision_calls = []

        def foreign_collision_cleanup(command, **kwargs):
            foreign_collision_calls.append(command)
            result = successful_compose_cleanup(command, **kwargs)
            if command[0:3] == ["docker", "container", "inspect"]:
                payload = json.loads(result.stdout)
                payload[0]["Mounts"][0]["Source"] = str(foreign_mount)
                return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")
            return result

        foreign_collision_artifact = root / "compose-cleanup-foreign-collision.json"
        try:
            cleanup_proof_cache(
                fake_cli,
                root,
                foreign_collision_root,
                foreign_collision_artifact,
                foreign_collision_cleanup,
            )
        except RuntimeError as exc:
            assert "foreign cache" in str(exc)
        else:
            raise AssertionError("foreign Compose collision must fail closed")
        assert foreign_collision_root.exists()
        assert not any("rm" in command for command in foreign_collision_calls)
        collision_cleanup = read_json_file(foreign_collision_artifact)
        assert collision_cleanup["removed"] is False
        assert any(
            item.get("kind") == "docker_resource_validation"
            for item in collision_cleanup["commands"]
        )
        remove_tree_with_retry(foreign_collision_root)

        no_compose_root = root / "no-compose-cleanup"
        write_proof_compose_state(no_compose_root, {"compose_file": None})
        no_compose_artifact = root / "no-compose-cleanup.json"
        cleanup_proof_cache(fake_cli, root, no_compose_root, no_compose_artifact)
        no_compose_cleanup = read_json_file(no_compose_artifact)
        assert no_compose_cleanup["removed"] is True
        assert no_compose_cleanup["commands"][0]["kind"] == "compose_down_skipped"

        symlink_root = root / "compose-validation-symlink"
        symlink_root.mkdir()
        symlink_identity = proof_agent_identity(symlink_root, root, PROOF_LOCAL_RUN_ID)
        Path(symlink_identity["state_file"]).parent.mkdir(parents=True)
        symlink_target = root / "compose-validation-symlink-target.json"
        write_json(symlink_target, {"owner": "codestory"})
        Path(symlink_identity["state_file"]).symlink_to(symlink_target)
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

        global_namespace_root = root / "compose-validation-global-local"
        global_namespace_root.mkdir()
        write_json(global_namespace_root / "retrieval-sidecars.json", {"owner": "codestory"})
        global_namespace_artifact = root / "compose-validation-global-local.json"
        try:
            cleanup_proof_cache(fake_cli, root, global_namespace_root, global_namespace_artifact)
        except RuntimeError as exc:
            assert "global local-sidecar namespace" in str(exc)
        else:
            raise AssertionError("proof cleanup must refuse the global codestory namespace")
        remove_tree_with_retry(global_namespace_root)

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
        registered_root = proof_owned_test_root("self-test")
        registered_archive = registered_root / archive.name
        shutil.copyfile(archive, registered_archive)
        registered_cache = registered_root / "codestory-packaged-proof-cache-self-test"
        write_proof_compose_state(
            registered_cache,
            {"compose_file": None},
            state_project=project,
        )
        registered_sidecars = proof_agent_identities([registered_cache], project)
        write_json(
            registered_root / PROOF_TEMP_OWNER_FILE,
            {
                "owner": "codestory-macos-metal-proof",
                "repository": os.environ.get("GITHUB_REPOSITORY"),
                "project": str(project),
                "cache_roots": [str(registered_cache)],
                "sidecars": registered_sidecars,
                "launches": [],
                "ports": [],
                "archive_name": registered_archive.name,
                "archive_sha256": sha256_file(registered_archive),
            },
        )
        registered_cleanup_out = root / "registered-cleanup-out"
        cleanup_registered_proof_temp_root(
            argparse.Namespace(
                project=str(project),
                out_dir=str(registered_cleanup_out),
                cleanup_proof_temp_root=str(registered_root),
            )
        )
        assert not registered_root.exists()
        registered_cleanup = read_json_file(registered_cleanup_out / "proof-owned-cleanup.json")
        assert registered_cleanup["cache_cleanup"][0]["status"] == "removed"
        registered_cache_cleanup = read_json_file(
            registered_cleanup_out / "registered-cache-cleanup-0.json"
        )
        assert any(
            item.get("kind") == "retrieval_down_skipped"
            and item.get("reason") == "trusted_direct_cleanup"
            for item in registered_cache_cleanup["commands"]
        )

        if sys.platform == "darwin":
            retry_root = proof_owned_test_root("missing-edge")
            retry_archive = retry_root / archive.name
            shutil.copyfile(archive, retry_archive)
            vanished_project = root / "vanished edge project"
            vanished_compose = vanished_project / "docker" / "retrieval-compose.yml"
            vanished_compose.parent.mkdir(parents=True)
            vanished_compose.write_text("services: {}\n", encoding="utf-8")
            missing_compose_cache = retry_root / "codestory-packaged-proof-cache-missing-compose"
            continuing_cache = retry_root / "codestory-packaged-proof-cache-continuing"
            probe_code = "import time; time.sleep(60)"
            cleanup_probe = subprocess.Popen(
                [sys.executable, "-c", probe_code, "--registered-cleanup-child"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            launch = {
                "launch_mode": "native_spawned",
                "pid": cleanup_probe.pid,
                "spawned_at_epoch_ms": int(time.time() * 1000),
                "executable_path": sys.executable,
                "launch_args": ["-c", probe_code, "--registered-cleanup-child"],
                "launch_fingerprint_sha256": "1" * 64,
            }
            write_proof_compose_state(
                missing_compose_cache,
                {
                    "compose_file": str(vanished_compose),
                    "embedding_launch": launch,
                    "embedding_launch_ownership": "owner",
                },
                state_project=vanished_project,
            )
            write_proof_compose_state(
                continuing_cache,
                {"compose_file": None},
                state_project=vanished_project,
            )
            retry_sidecars = proof_agent_identities(
                [missing_compose_cache, continuing_cache], vanished_project
            )
            missing_identity = next(
                identity
                for identity in retry_sidecars
                if identity["cache_root"] == str(missing_compose_cache.resolve())
                and identity["run_id"] == PROOF_LOCAL_RUN_ID
            )
            missing_identity["docker_resources"] = {
                "compose_project": missing_identity["compose_project"],
                "containers": [],
                "networks": [],
            }
            write_json(
                retry_root / PROOF_TEMP_OWNER_FILE,
                {
                    "owner": "codestory-macos-metal-proof",
                    "repository": os.environ.get("GITHUB_REPOSITORY"),
                    "project": str(vanished_project),
                    "cache_roots": [str(missing_compose_cache), str(continuing_cache)],
                    "sidecars": retry_sidecars,
                    "launches": [launch],
                    "ports": [],
                    "archive_name": retry_archive.name,
                    "archive_sha256": sha256_file(retry_archive),
                },
            )
            remove_tree_with_retry(vanished_project)

            def empty_registered_docker(command, **_kwargs):
                if command[0:3] in (
                    ["docker", "container", "ls"],
                    ["docker", "network", "ls"],
                ):
                    return subprocess.CompletedProcess(command, 0, "[]", "")
                raise AssertionError(f"unexpected registered cleanup Docker command: {command}")

            retry_out = root / "registered-cleanup-missing-edge-out"
            try:
                cleanup_registered_proof_temp_root(
                    argparse.Namespace(
                        project=str(vanished_project),
                        out_dir=str(retry_out),
                        cleanup_proof_temp_root=str(retry_root),
                        run=empty_registered_docker,
                    )
                )
            finally:
                if cleanup_probe.poll() is None:
                    cleanup_probe.terminate()
                    cleanup_probe.wait(timeout=5)
            assert cleanup_probe.poll() is not None
            retry_cleanup = read_json_file(retry_out / "proof-owned-cleanup.json")
            assert retry_cleanup["root_removed"] is True
            assert [item["status"] for item in retry_cleanup["cache_cleanup"]] == [
                "removed",
                "removed",
            ]
            assert retry_cleanup["native_processes"][0]["status"] == "terminated"

        tampered_root = proof_owned_test_root("tampered")
        tampered_archive = tampered_root / archive.name
        shutil.copyfile(archive, tampered_archive)
        tampered_cache = tampered_root / "codestory-packaged-proof-cache-tampered"
        write_proof_compose_state(
            tampered_cache,
            {"compose_file": None},
            state_project=project,
        )
        write_json(
            tampered_root / PROOF_TEMP_OWNER_FILE,
            {
                "owner": "codestory-macos-metal-proof",
                "repository": os.environ.get("GITHUB_REPOSITORY"),
                "project": str(project),
                "cache_roots": [str(tampered_cache)],
                "sidecars": proof_agent_identities([tampered_cache], project),
                "launches": [],
                "ports": [],
                "archive_name": tampered_archive.name,
                "archive_sha256": "0" * 64,
            },
        )
        try:
            cleanup_registered_proof_temp_root(
                argparse.Namespace(
                    project=str(project),
                    out_dir=str(root / "tampered-cleanup-out"),
                    cleanup_proof_temp_root=str(tampered_root),
                )
            )
        except RuntimeError as exc:
            assert "archive digest changed" in str(exc)
        else:
            raise AssertionError("stale cleanup must reject a modified bound archive")
        assert tampered_cache.exists()
        remove_tree_with_retry(tampered_root)
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
        full_cache_root = stdio_artifact["status"]["cache_root"]
        assert Path(full_cache_root).name.startswith("codestory-packaged-proof-cache-")
        status_request = next(
            entry["request"]
            for entry in stdio_artifact["transcript"]
            if entry["request"].get("id") == "status"
        )
        assert status_request["params"]["project"] == str(project.resolve())

        expect_fake_gate_failure(
            args,
            "search",
            "search",
            "search.json.stdout.txt",
            "forced fake search failure should fail the gate",
        )

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
    parser.add_argument(
        "--native-accelerator-lifecycle",
        action="store_true",
        help="Require native accelerated runtime survival, dead-endpoint blocking, and repair recovery during the full proof.",
    )
    parser.add_argument(
        "--native-edge-cases",
        action="store_true",
        help="Exercise spaces/Unicode paths and corrupt managed native installs.",
    )
    parser.add_argument(
        "--intel-runtime-policy",
        action="store_true",
        help="On Intel macOS, prove default backend failure plus explicitly labelled CPU/external operation without Metal claims.",
    )
    parser.add_argument(
        "--cleanup-proof-temp-root",
        help="Clean only a marker-owned hardware-proof temp root and verify its registered processes and ports are gone.",
    )
    parser.add_argument("--self-test", action="store_true", help="Run script self-tests.")
    args = parser.parse_args()
    if not args.self_test and not args.cleanup_proof_temp_root and not args.archive:
        parser.error("--archive is required unless --self-test or cleanup mode is set")
    if not args.self_test and not args.cleanup_proof_temp_root and not args.expected_version:
        parser.error("--expected-version is required unless --self-test or cleanup mode is set")
    if args.managed_plugin_handoff and not args.plugin_root:
        parser.error("--plugin-root is required with --managed-plugin-handoff")
    if args.managed_plugin_handoff and args.version_only:
        parser.error("--managed-plugin-handoff and --version-only are mutually exclusive")
    if args.intel_runtime_policy and (args.managed_plugin_handoff or args.version_only):
        parser.error("--intel-runtime-policy is mutually exclusive with --managed-plugin-handoff and --version-only")
    if args.native_accelerator_lifecycle and (args.managed_plugin_handoff or args.version_only):
        parser.error("--native-accelerator-lifecycle requires the full packaged proof")
    if args.native_accelerator_lifecycle and args.intel_runtime_policy:
        parser.error("--native-accelerator-lifecycle and --intel-runtime-policy are mutually exclusive")
    if args.native_accelerator_lifecycle and os.name == "nt":
        parser.error("--native-accelerator-lifecycle is supported only on POSIX hosts")
    if args.native_edge_cases and not args.native_accelerator_lifecycle:
        parser.error("--native-edge-cases requires --native-accelerator-lifecycle")
    if args.cleanup_proof_temp_root and (
        args.native_accelerator_lifecycle
        or args.native_edge_cases
        or args.intel_runtime_policy
        or args.managed_plugin_handoff
        or args.version_only
    ):
        parser.error("--cleanup-proof-temp-root is a standalone cleanup mode")
    return args


def main() -> None:
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if args.cleanup_proof_temp_root:
        cleanup_registered_proof_temp_root(args)
        return
    try:
        run_gate(args)
    except GateFailure as exc:
        print(f"::error::layer={exc.layer} artifact={exc.artifact} {exc.message}", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
