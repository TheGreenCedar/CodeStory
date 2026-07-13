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
import time
import zipfile
from collections.abc import Callable
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
SIDECAR_STATE_FILE_V3 = "retrieval-sidecars-v3.json"
LEGACY_SIDECAR_STATE_FILE = "retrieval-sidecars.json"


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
    state_file = cache_root / SIDECAR_STATE_FILE_V3
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


def make_repository_convergence_copy_stale(project: Path) -> Path:
    probe = project / "crates" / "codestory-cli" / "src" / "managed_convergence_probe.rs"
    if not probe.parent.is_dir():
        raise RuntimeError(
            f"managed convergence proof cannot locate codestory-cli sources under {project}"
        )
    probe.write_text(
        "pub fn discovered_by_grounding_convergence() -> &'static str { \"fresh\" }\n",
        encoding="utf-8",
    )
    return probe


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


def fnv1a_bytes(value: bytes) -> str:
    digest = 0xCBF29CE484222325
    for byte in value:
        digest ^= byte
        digest = (digest * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{digest:016x}"


def fnv1a_hex(value: str) -> str:
    return fnv1a_bytes(os.fsencode(value))


def workspace_id_v3_for_path(path: Path) -> str:
    encoded = str(path).encode("utf-16le") if os.name == "nt" else os.fsencode(path)
    return fnv1a_bytes(encoded)


def proof_agent_identity(cache_root: Path, project: Path, run_id: str) -> dict[str, str]:
    canonical_cache = cache_root.resolve(strict=False)
    canonical_project = project.resolve(strict=False)
    workspace_id = workspace_id_v3_for_path(canonical_project)
    namespace = f"codestory-agent-v3-{workspace_id}-{run_id}"
    state_root = canonical_cache / "sidecars" / namespace
    return {
        "cache_root": str(canonical_cache),
        "project": str(canonical_project),
        "profile": "agent",
        "run_id": run_id,
        "namespace": namespace,
        "compose_project": namespace,
        "state_file": str(state_root / SIDECAR_STATE_FILE_V3),
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
    candidate = value.strip()
    if "." in candidate:
        prefix, fraction_and_zone = candidate.rsplit(".", 1)
        fraction_length = next(
            (
                index
                for index, character in enumerate(fraction_and_zone)
                if not character.isdigit()
            ),
            len(fraction_and_zone),
        )
        candidate = (
            f"{prefix}.{fraction_and_zone[:6]}"
            f"{fraction_and_zone[fraction_length:]}"
        )
    if candidate.endswith("Z"):
        candidate = candidate[:-1] + "+00:00"
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
    for local_state in (
        cache_root / SIDECAR_STATE_FILE_V3,
        cache_root / LEGACY_SIDECAR_STATE_FILE,
    ):
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
    if expected["namespace"] == "codestory-v3" or not expected["namespace"].startswith(
        "codestory-agent-v3-"
    ):
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
        for path in (canonical_cache / "sidecars").glob(f"*/{SIDECAR_STATE_FILE_V3}")
    }
    unexpected = sorted(str(path) for path in discovered_state_files - expected_state_files)
    if unexpected:
        error = f"proof cache contains unregistered sidecar state: {unexpected}"
        results.append({"kind": "compose_state_validation", "error": error})
        errors.append(error)
    worker_cleanup, worker_errors = cleanup_proof_owned_repair_workers(
        canonical_cache,
        project,
        identities,
    )
    results.extend(worker_cleanup)
    errors.extend(worker_errors)
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


def seed_stale_local_publication(
    cli: Path,
    project: Path,
    out_dir: Path,
    timeout_secs: int,
    env: dict[str, str],
    make_stale: Callable[[Path], object],
) -> dict[str, str]:
    ready_artifact = out_dir / "managed-local-ready.json"
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
            str(ready_artifact),
        ],
        ready_artifact,
        timeout_secs,
        env=env,
    )

    ground_artifact = out_dir / "managed-local-ground.json"
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
            str(ground_artifact),
        ],
        ground_artifact,
        timeout_secs,
        env=env,
    )
    require(
        isinstance(ground, dict) and isinstance(ground.get("stats"), dict),
        "managed_local_ground",
        ground_artifact,
        "managed local ground output is missing repository stats",
    )
    stale_result = make_stale(project)
    stale_artifact = out_dir / "managed-local-stale.json"
    write_json(stale_artifact, {"project": str(project), "mutation": str(stale_result)})
    return {
        "managed_local_ready": str(ready_artifact),
        "managed_local_ground": str(ground_artifact),
        "managed_local_stale": str(stale_artifact),
    }


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


def process_start_identity_snapshot(
    pid: int,
    run=subprocess.run,
    *,
    platform: str | None = None,
    system: str | None = None,
) -> tuple[str, str | None]:
    platform = platform or os.name
    system = system or sys.platform
    if pid <= 0:
        return "invalid_pid", None
    if system.startswith("linux"):
        try:
            stat_text = Path("/proc", str(pid), "stat").read_text(encoding="utf-8")
        except FileNotFoundError:
            return "already_exited", None
        except OSError:
            return "unknown", None
        fields = stat_text.rsplit(") ", 1)
        start = fields[1].split()[19] if len(fields) == 2 and len(fields[1].split()) > 19 else None
        return ("running", f"linux:{start}") if start else ("unknown", None)
    if platform == "nt":
        script = f'$p=Get-CimInstance Win32_Process -Filter "ProcessId = {pid}" -ErrorAction Stop; if ($null -eq $p) {{ exit 2 }}; $p.CreationDate.ToUniversalTime().Ticks'
        command, prefix, gone = ["powershell", "-NoProfile", "-NonInteractive", "-Command", script], "windows:", 2
    else:
        command, prefix, gone = ["ps", "-o", "lstart=", "-p", str(pid)], "unix:", 1
    try:
        result = run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=2,
            check=False,
            text=True,
            env={**os.environ, "LC_ALL": "C", "TZ": "UTC"},
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown", None
    if result.returncode == gone:
        return "already_exited", None
    identity = result.stdout.strip()
    return ("running", f"{prefix}{identity}") if result.returncode == 0 and identity else ("unknown", None)


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
    expected_start_identity: str | None = None,
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
    if expected_start_identity is not None:
        identity_status, actual_start_identity = process_start_identity_snapshot(pid)
        diagnostics["process_identity"] = {
            "status": identity_status,
            "start_identity": actual_start_identity,
        }
        if identity_status == "already_exited":
            diagnostics["status"] = "already_exited"
            return
        if identity_status != "running":
            raise RuntimeError(f"could not prove worker process {pid} start identity")
        if actual_start_identity != expected_start_identity:
            raise RuntimeError(f"refusing to terminate reused worker pid {pid}")
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
                waited_pid, _status = os.waitpid(pid, os.WNOHANG)
                if waited_pid == pid:
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


def cleanup_proof_owned_repair_workers(
    cache_root: Path,
    project: Path,
    registered_sidecars: list[dict] | None = None,
) -> tuple[list[dict], list[str]]:
    evidence = []
    errors = []
    canonical_cache = cache_root.resolve(strict=False)
    identities = registered_sidecars or proof_agent_identities([canonical_cache], project)
    for identity in identities:
        item = None
        try:
            if (
                not isinstance(identity, dict)
                or Path(str(identity.get("cache_root", ""))).resolve(strict=False) != canonical_cache
            ):
                continue
            path = Path(identity["state_file"]).with_name("ready-repair-enqueue.lock")
            if not path.exists():
                continue
            if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(canonical_cache):
                raise RuntimeError(f"proof ready-repair reservation is unsafe: {path}")
            record = read_json_file(path)
            if not isinstance(record, dict):
                raise RuntimeError(f"proof ready-repair reservation is invalid: {path}")
            if record.get("adopted") is not True:
                continue
            project_root = record.get("project_root")
            expected = (identity["project"], identity["profile"], identity["run_id"], identity["namespace"])
            observed = (
                str(Path(project_root).resolve(strict=False)) if isinstance(project_root, str) else None,
                record.get("profile"),
                record.get("run_id"),
                record.get("namespace"),
            )
            pid = record.get("pid")
            attempt = record.get("token")
            start = record.get("process_start_identity")
            if observed != expected or not isinstance(pid, int) or pid <= 0:
                raise RuntimeError("proof ready-repair reservation ownership changed")
            if not isinstance(attempt, str) or not attempt or not isinstance(start, str) or not start:
                raise RuntimeError("proof ready-repair reservation process identity is incomplete")
            item = {
                "kind": "ready_repair_worker_cleanup",
                "pid": pid,
                "attempt_id": attempt,
                "process_start_identity": start,
                "source": path.name,
            }
            evidence.append(item)
            terminate_worker_pid(
                pid,
                item,
                expected_start_identity=start,
            )
        except Exception as exc:
            error = f"{type(exc).__name__}: {exc}"
            if item is None:
                evidence.append({
                    "kind": "ready_repair_worker_validation",
                    "state_file": str(identity.get("state_file", "")) if isinstance(identity, dict) else "",
                    "error": error,
                })
            else:
                item["error"] = error
            errors.append(error)
    return evidence, errors


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


def managed_convergence_request_plan(
    project: Path,
    require_ready: bool,
    question: str,
    query: str,
) -> dict:
    activation = {
        "jsonrpc": "2.0",
        "id": "ground_activation",
        "method": "tools/call",
        "params": {
            "name": "ground",
            "arguments": {"project": str(project), "budget": "strict"},
        },
    }
    if not require_ready:
        return {
            "extra_requests": [
                activation,
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
            ]
        }
    return {
        "extra_requests": [activation],
        "poll_request": {
            "jsonrpc": "2.0",
            "id": "convergence_status",
            "method": "resources/read",
            "params": {"uri": STATUS_URI, "project": str(project)},
        },
        "poll_until": managed_status_response_is_ready,
        "poll_interval_secs": 2,
        "post_poll_requests": [
            {
                "jsonrpc": "2.0",
                "id": "packet_after_convergence",
                "method": "tools/call",
                "params": {
                    "name": "packet",
                    "arguments": {"project": str(project), "question": question, "budget": "compact"},
                },
            },
            {
                "jsonrpc": "2.0",
                "id": "search_after_convergence",
                "method": "tools/call",
                "params": {
                    "name": "search",
                    "arguments": {"project": str(project), "query": query, "why": True},
                },
            },
        ],
    }


def plugin_stdio_handoff(
    plugin_root: Path,
    release_dir: Path,
    project: Path,
    cache_root: Path,
    artifact: Path,
    timeout_secs: int,
    expected_version: str,
    archive_cli: Path,
    model_dir: Path,
    archive: Path,
    *,
    require_ready: bool = False,
    question: str = DEFAULT_QUESTION,
    query: str = DEFAULT_QUERY,
) -> dict:
    require_plugin_manifest_version(plugin_root, expected_version)
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(
        launcher.is_file(),
        "managed_plugin_convergence",
        artifact,
        f"plugin launcher is missing: {launcher}",
    )
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
            "CODESTORY_EMBED_MODEL_DIR": str(model_dir),
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
        request_plan = managed_convergence_request_plan(project, require_ready, question, query)
        status = stdio_status_command(
            ["node", str(launcher)],
            artifact,
            timeout_secs,
            project,
            layer="managed_plugin_convergence",
            cwd=project,
            env=plugin_env,
            **request_plan,
            cleanup_status_workers=True,
        )
        if require_ready:
            status = require_managed_plugin_ready_convergence(
                status,
                artifact,
                expected_version,
                archive_cli,
            )
        else:
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
    poll_request: dict | None = None,
    poll_until: Callable[[dict], bool] | None = None,
    poll_interval_secs: float = 1.0,
    post_poll_requests: list[dict] | None = None,
    cleanup_status_workers: bool = False,
) -> dict:
    if (poll_request is None) != (poll_until is None):
        raise ValueError("poll_request and poll_until must be provided together")
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

    def send_request(request_spec: dict) -> dict:
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
            send_request(request_spec)

        if poll_request is not None and poll_until is not None:
            poll_deadline = time.monotonic() + timeout_secs
            poll_id = str(poll_request.get("id", "poll"))
            attempt = 0
            while True:
                if attempt:
                    remaining = poll_deadline - time.monotonic()
                    if remaining <= 0:
                        stdio_fail(f"stdio MCP poll {poll_id} timed out after {timeout_secs}s", {"timed_out": True})
                    time.sleep(min(poll_interval_secs, remaining))
                attempt += 1
                request = {**poll_request, "id": f"{poll_id}_{attempt}"}
                if poll_until(send_request(request)):
                    break
                if time.monotonic() >= poll_deadline:
                    stdio_fail(f"stdio MCP poll {poll_id} timed out after {timeout_secs}s", {"timed_out": True})

        for request_spec in post_poll_requests or []:
            send_request(request_spec)
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
        cache_value = (env or os.environ).get("CODESTORY_CACHE_ROOT")
        worker_cleanup, worker_errors = (
            cleanup_proof_owned_repair_workers(Path(cache_value), project)
            if cleanup_status_workers and cache_value
            else ([], [])
        )
        write_json(worker_cleanup_artifact, {"workers": worker_cleanup})
        if worker_errors:
            raise RuntimeError("proof-owned repair worker cleanup failed: " + "; ".join(worker_errors))
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


def resource_statuses(responses: list[dict], request_prefix: str) -> list[dict]:
    return [
        status
        for response in responses
        if str(response.get("id", "")).startswith(request_prefix)
        and isinstance(status := status_from_resource_response(response), dict)
    ]


def managed_status_is_ready(status: dict) -> bool:
    surfaces = status.get("allowed_surfaces", {})
    proof = status.get("readiness_broker", {}).get("gpu_proof", {})
    return (
        local_freshness_status(status) == "fresh"
        and status.get("retrieval_mode") == "full"
        and surfaces.get("packet", {}).get("allowed") is True
        and surfaces.get("search", {}).get("allowed") is True
        and proof.get("proof_status") == "verified"
        and proof.get("meaningful_accelerator_work_proven") is True
        and proof.get("embed_smoke_ok") is True
        and proof.get("requested_provider") == "metal"
        and proof.get("detected_provider") == "metal"
    )


def managed_status_response_is_ready(response: dict) -> bool:
    status = status_from_resource_response(response)
    return isinstance(status, dict) and managed_status_is_ready(status)


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


def require_single_managed_repair(
    setup_snapshots: list[dict],
    expected_project: str | None,
    expected_outcomes: set[str],
    artifact: Path,
) -> dict:
    records = [
        record
        for setup in setup_snapshots
        for record in (setup.get("active_repair"), setup.get("last_worker_result"))
        if isinstance(record, dict)
        and isinstance(record.get("attempt_id"), str)
        and record.get("attempt_id")
    ]
    attempt_ids = {record["attempt_id"] for record in records}
    namespaces = [record.get("namespace") for record in records]
    require(
        len(attempt_ids) == 1,
        "managed_plugin_convergence",
        artifact,
        f"ground activation did not retain exactly one repair attempt: {sorted(attempt_ids)}",
    )
    require(
        isinstance(expected_project, str)
        and expected_project
        and all(
            record.get("project_root") == expected_project
            and record.get("profile") == "agent"
            and record.get("run_id") == "shared-agent"
            and isinstance(record.get("namespace"), str)
            and record.get("namespace")
            for record in records
        )
        and len(set(namespaces)) == 1,
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
        and terminal.get("outcome") in expected_outcomes,
        "managed_plugin_convergence",
        artifact,
        f"automatic shared-agent repair did not finish with an expected outcome: {sorted(expected_outcomes)}",
    )
    return terminal


def require_managed_ground_activation(
    status_before: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> int:
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
    return initial_generation


def require_managed_generation_advanced(
    status_after: dict | None,
    initial_generation: int,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> dict:
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
    return status_after


def require_managed_plugin_convergence(
    status_before: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> None:
    initial_generation = require_managed_ground_activation(
        status_before,
        artifact,
        expected_version,
        archive_cli,
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
    require_single_managed_repair(
        setup_snapshots,
        status_before.get("project_root"),
        {"failed", "succeeded"},
        artifact,
    )
    status_after = status_from_resource_response(
        transcript_response(artifact, "status_after_convergence") or {}
    )
    status_after = require_managed_generation_advanced(
        status_after,
        initial_generation,
        artifact,
        expected_version,
        archive_cli,
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


def require_managed_plugin_ready_convergence(
    status_before: dict,
    artifact: Path,
    expected_version: str,
    archive_cli: Path,
) -> dict:
    initial_generation = require_managed_ground_activation(
        status_before,
        artifact,
        expected_version,
        archive_cli,
    )

    transcript = read_json_file(artifact).get("transcript", [])
    tool_names = [
        entry.get("request", {}).get("params", {}).get("name")
        for entry in transcript
        if entry.get("request", {}).get("method") == "tools/call"
    ]
    require(
        tool_names == ["ground", "packet", "search"],
        "managed_plugin_convergence",
        artifact,
        f"grounding-only proof invoked unexpected MCP tools: {tool_names}",
    )
    status_snapshots = resource_statuses(
        [entry.get("response", {}) for entry in transcript],
        "convergence_status_",
    )
    require(
        status_snapshots and managed_status_is_ready(status_snapshots[-1]),
        "managed_plugin_convergence",
        artifact,
        "grounding activation did not reach full verified packet/search readiness",
    )

    require_single_managed_repair(
        [status.get("sidecar_setup", {}) for status in status_snapshots],
        status_before.get("project_root"),
        {"succeeded"},
        artifact,
    )

    for status in [status_before, *status_snapshots]:
        proof = status.get("readiness_broker", {}).get("gpu_proof", {})
        if proof.get("proof_status") == "verified" and proof.get("embed_smoke_ok") is True:
            continue
        surfaces = status.get("allowed_surfaces", {})
        require(
            surfaces.get("packet", {}).get("allowed") is False
            and surfaces.get("search", {}).get("allowed") is False,
            "managed_plugin_convergence",
            artifact,
            "packet/search opened before verified GPU and embedding proof",
        )

    status_after = require_managed_generation_advanced(
        status_snapshots[-1],
        initial_generation,
        artifact,
        expected_version,
        archive_cli,
    )
    proof = status_after.get("readiness_broker", {}).get("gpu_proof", {})
    launch = proof.get("runtime_identity", {}).get("embedding_launch")
    resource = status_after.get("readiness_broker", {}).get("resources", {}).get("native_embedding_runtime", {})
    require(
        managed_status_is_ready(status_after)
        and proof.get("observation_source") == "native_log"
        and isinstance(launch, dict)
        and launch.get("launch_mode") == "native_spawned"
        and isinstance(launch.get("pid"), int)
        and resource.get("owner_pid") == launch.get("pid"),
        "managed_plugin_convergence",
        artifact,
        "full retrieval did not retain verified matching native Metal runtime identity",
    )

    packet = structured_content_from_response(transcript_response(artifact, "packet_after_convergence") or {})
    search = structured_content_from_response(transcript_response(artifact, "search_after_convergence") or {})
    require_packet_ready(packet, artifact)
    require_search_full(search, artifact)
    return status_after


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
        grounding_convergence = getattr(args, "managed_plugin_grounding_convergence", False)
        if grounding_convergence:
            edge_project = Path(temp) / "CodeStory managed grounding convergence"
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
            summary["artifacts"].update(
                seed_stale_local_publication(
                    cli,
                    convergence_project,
                    out_dir,
                    args.timeout_secs,
                    proof_env,
                    make_managed_convergence_fixture_stale,
                )
            )

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
        if grounding_convergence:
            summary["artifacts"].update(
                seed_stale_local_publication(
                    cli,
                    project,
                    out_dir,
                    args.timeout_secs,
                    proof_env,
                    make_repository_convergence_copy_stale,
                )
            )
            model_dir = os.environ.get("CODESTORY_EMBED_MODEL_DIR", "").strip()
            require(
                bool(model_dir) and Path(model_dir).is_dir(),
                "managed_plugin_convergence",
                archive,
                "grounding convergence requires a prepared CODESTORY_EMBED_MODEL_DIR",
            )
            plugin_artifact = out_dir / "managed-plugin-ready-convergence.json"
            plugin_status = plugin_stdio_handoff(
                Path(args.plugin_root).resolve(),
                archive.parent,
                project,
                cache_root,
                plugin_artifact,
                args.timeout_secs,
                args.expected_version,
                cli,
                Path(model_dir),
                archive,
                require_ready=True,
                question=args.question,
                query=args.query,
            )
            register_current_proof_runtime(cache_root)
            summary["artifacts"]["managed_plugin_ready_convergence"] = str(plugin_artifact)
            summary["artifacts"]["managed_plugin_upgrade"] = str(
                plugin_artifact.with_name("managed-plugin-upgrade.json")
            )

            restart_artifact = out_dir / "managed-plugin-restart-status.json"
            restart_status = plugin_stdio_status(
                Path(args.plugin_root).resolve(),
                archive.parent,
                project,
                restart_artifact,
                args.timeout_secs,
                args.expected_version,
                cache_root,
            )
            require_plugin_stdio_ready(restart_status, restart_artifact, args.expected_version)
            first_launch = (
                plugin_status.get("readiness_broker", {})
                .get("gpu_proof", {})
                .get("runtime_identity", {})
                .get("embedding_launch", {})
            )
            restart_launch = (
                restart_status.get("readiness_broker", {})
                .get("gpu_proof", {})
                .get("runtime_identity", {})
                .get("embedding_launch", {})
            )
            require(
                isinstance(first_launch, dict)
                and isinstance(restart_launch, dict)
                and restart_launch.get("pid") == first_launch.get("pid")
                and restart_launch.get("launch_fingerprint_sha256")
                == first_launch.get("launch_fingerprint_sha256"),
                "managed_plugin_restart",
                restart_artifact,
                "restarted MCP did not reuse the exact healthy native embedding runtime",
            )
            summary["artifacts"]["managed_plugin_restart"] = str(restart_artifact)
            ready_args = [
                "ready",
                "--goal",
                "agent",
                "--project",
                str(project),
                "--format",
                "json",
                "--output-file",
                str(ready_artifact),
            ]
        else:
            ready_args = [
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
            ]
        ready = run_command(
            cli,
            "ready",
            ready_args,
            ready_artifact,
            args.timeout_secs,
            env=local_env,
        )
        require_agent_ready(ready, "ready", ready_artifact)
        register_current_proof_runtime(cache_root)
        summary["artifacts"]["ready"] = str(ready_artifact)
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


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-self-test-") as temp:
        root = Path(temp)
        artifact = root / "validator.json"
        assert fnv1a_bytes(r"C:\CodeStory".encode("utf-16le")) == "bf15b1ad4ccb5afe"
        runner_temp = os.environ.get("RUNNER_TEMP", "").strip()
        proof_root_parent = Path(runner_temp).resolve(strict=True) if runner_temp else root

        assert docker_created_epoch_ms(
            "2026-07-13T14:08:36.245344828Z"
        ) == docker_created_epoch_ms("2026-07-13T14:08:36.245344Z")

        require_agent_not_ready(
            {
                "verdicts": [{
                    "goal": "agent_packet_search",
                    "status": "repair_retrieval",
                }],
                "readiness_broker": {
                    "gpu_proof": {"proof_status": "gpu_unverified"}
                },
            },
            "native_runtime_dead_status",
            artifact,
        )

        with embedding_probe_server() as endpoint:
            port = int(endpoint.split(":", 2)[2].split("/", 1)[0])
            assert port_reachability(port)
            require_intel_cpu_external_ready(
                {
                    "compose_started": False,
                    "embed_reachable": True,
                    "sidecar_state": {
                        "embed_url": endpoint,
                        "embedding_device_policy": "cpu_allowed",
                        "embedding_device_state": "cpu",
                        "embedding_device_observation_source": "cpu_policy",
                        "embedding_cpu_allowed": True,
                        "embedding_accelerator_requested": False,
                        "embedding_accelerator_request_provider": None,
                    },
                },
                artifact,
                endpoint,
            )

        if sys.platform == "darwin":
            probe_executable = "/bin/sleep"
            probe_arguments = ["60"]
            process_probe = subprocess.Popen(
                [probe_executable, *probe_arguments],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                identity_deadline = time.monotonic() + 2
                while True:
                    observed_process = darwin_process_argv(process_probe.pid)
                    if observed_process is not None:
                        break
                    if process_probe.poll() is not None or time.monotonic() >= identity_deadline:
                        raise AssertionError("Darwin process identity did not stabilize")
                    time.sleep(0.05)
                launch = {
                    "pid": process_probe.pid,
                    "spawned_at_epoch_ms": int(time.time() * 1000),
                    "executable_path": observed_process[0],
                    "launch_args": probe_arguments,
                }
                assert registered_native_process_snapshot(launch)["status"] == "matching"
                wrong_argv = {
                    **launch,
                    "launch_args": ["6"],
                }
                assert (
                    registered_native_process_snapshot(wrong_argv)["status"]
                    == "identity_mismatch"
                )
            finally:
                process_probe.terminate()
                process_probe.wait(timeout=5)

        compose_file = root / "docker" / "retrieval-compose.yml"
        compose_file.parent.mkdir()
        compose_file.write_text("services: {}\n", encoding="utf-8")

        def write_proof_compose_state(
            cache_root: Path,
            overrides: dict | None = None,
            *,
            state_project: Path = root,
        ) -> dict[str, str]:
            cache_root.mkdir()
            identity = proof_agent_identity(
                cache_root, state_project, PROOF_LOCAL_RUN_ID
            )
            state_root = Path(identity["state_file"]).parent
            for name in ("qdrant", "lexical", "scip"):
                (state_root / name).mkdir(parents=True, exist_ok=True)
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
            write_json(Path(identity["state_file"]), state)
            return identity

        if os.name != "nt":
            worker_cache = root / "ready-repair-cleanup"
            identity = proof_agent_identity(worker_cache, root, PROOF_LOCAL_RUN_ID)
            Path(identity["state_file"]).parent.mkdir(parents=True)
            worker = subprocess.Popen(
                [sys.executable, "-c", "import time; time.sleep(60)"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                status, start = process_start_identity_snapshot(worker.pid)
                assert status == "running" and start
                try:
                    terminate_worker_pid(
                        worker.pid,
                        expected_start_identity=f"{start}-reused",
                    )
                except RuntimeError as exc:
                    assert "reused worker pid" in str(exc)
                else:
                    raise AssertionError("PID reuse must fail closed")
                write_json(
                    Path(identity["state_file"]).with_name(
                        "ready-repair-enqueue.lock"
                    ),
                    {
                        "project_root": identity["project"],
                        "profile": identity["profile"],
                        "run_id": identity["run_id"],
                        "namespace": identity["namespace"],
                        "pid": worker.pid,
                        "token": "proof-ground-activation",
                        "process_start_identity": start,
                        "adopted": True,
                    },
                )
                cleanup, errors = cleanup_proof_owned_repair_workers(
                    worker_cache, root, [identity]
                )
                worker.wait(timeout=5)
                assert not errors and cleanup[0]["status"] == "terminated"
            finally:
                if worker.poll() is None:
                    worker.terminate()
                    worker.wait(timeout=5)

        compose_cache = root / "compose-cleanup"
        identity = write_proof_compose_state(compose_cache)
        assert identity["namespace"].startswith("codestory-agent-v3-")
        assert Path(identity["state_file"]).name == SIDECAR_STATE_FILE_V3
        compose_calls = []

        def fake_docker(command, **kwargs):
            compose_calls.append(command)
            namespace = kwargs["env"]["CODESTORY_SIDECAR_NAMESPACE"]
            qdrant_root = kwargs["env"]["CODESTORY_QDRANT_DATA_DIR"]
            container_id = f"container-{fnv1a_hex(qdrant_root)}"
            network_id = f"network-{fnv1a_hex(qdrant_root)}"
            if command[1:3] == ["container", "ls"]:
                stdout = json.dumps({"ID": container_id})
            elif command[1:3] == ["network", "ls"]:
                stdout = json.dumps({"ID": network_id})
            elif command[1:3] == ["container", "inspect"]:
                stdout = json.dumps([{
                    "Id": container_id,
                    "Name": f"/{namespace}-qdrant",
                    "Created": "2026-07-12T12:00:00Z",
                    "Config": {"Labels": {
                        "com.docker.compose.project": namespace,
                        "com.docker.compose.service": "qdrant",
                        "dev.codestory.owner": "codestory",
                        "dev.codestory.profile": "agent",
                        "dev.codestory.namespace": namespace,
                    }},
                    "Mounts": [{
                        "Type": "bind",
                        "Source": qdrant_root,
                        "Destination": "/qdrant/storage",
                    }],
                }])
            elif command[1:3] == ["network", "inspect"]:
                stdout = json.dumps([{
                    "Id": network_id,
                    "Name": f"{namespace}_default",
                    "Created": "2026-07-12T12:00:00Z",
                    "Labels": {
                        "com.docker.compose.project": namespace,
                        "com.docker.compose.network": "default",
                    },
                    "Containers": {
                        container_id: {"Name": f"{namespace}-qdrant"}
                    },
                }])
            else:
                stdout = "removed"
            return subprocess.CompletedProcess(command, 0, stdout, "")

        cleanup_artifact = root / "compose-cleanup.json"
        cleanup_proof_cache(
            None,
            root,
            compose_cache,
            cleanup_artifact,
            fake_docker,
            registered_sidecars=[identity],
        )
        assert not compose_cache.exists()
        assert [command[1] for command in compose_calls if "rm" in command] == [
            "container",
            "network",
        ]

        for state_file_name in (SIDECAR_STATE_FILE_V3, LEGACY_SIDECAR_STATE_FILE):
            global_cache = root / f"global-local-cache-{state_file_name}"
            global_cache.mkdir()
            write_json(global_cache / state_file_name, {"owner": "codestory"})
            try:
                cleanup_proof_cache(
                    None, root, global_cache, root / "global-local-cleanup.json"
                )
            except RuntimeError as exc:
                assert "global local-sidecar namespace" in str(exc)
            else:
                raise AssertionError("proof cleanup must refuse the global namespace")
            remove_tree_with_retry(global_cache)

        registered_root = (
            proof_root_parent
            / f"codestory-metal-proof-owned-self-test-{root.name}"
        )
        registered_root.mkdir()
        project = root / "repo"
        project.mkdir()
        archive = registered_root / "codestory-cli-v9.9.9-macos-arm64.tar.gz"
        archive.write_bytes(b"packaged proof")
        registered_cache = registered_root / "codestory-packaged-proof-cache-self-test"
        registered_identity = write_proof_compose_state(
            registered_cache,
            {"compose_file": None},
            state_project=project,
        )
        write_json(
            registered_root / PROOF_TEMP_OWNER_FILE,
            {
                "owner": "codestory-macos-metal-proof",
                "repository": os.environ.get("GITHUB_REPOSITORY"),
                "project": str(project),
                "cache_roots": [str(registered_cache)],
                "sidecars": [registered_identity],
                "launches": [],
                "ports": [],
                "archive_name": archive.name,
                "archive_sha256": sha256_file(archive),
            },
        )
        cleanup_out = root / "registered-cleanup-out"
        cleanup_registered_proof_temp_root(
            argparse.Namespace(
                project=str(project),
                out_dir=str(cleanup_out),
                cleanup_proof_temp_root=str(registered_root),
            )
        )
        assert not registered_root.exists()
        cleanup = read_json_file(cleanup_out / "proof-owned-cleanup.json")
        assert cleanup["root_removed"] is True
        assert cleanup["cache_cleanup"][0]["status"] == "removed"

    print("self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Gate releases on packaged full-sidecar agent proof.")
    parser.add_argument("--archive", help="Packaged codestory-cli archive to test.")
    parser.add_argument("--project", default=".", help="Representative repository to prove against.")
    parser.add_argument("--out-dir", default="target/packaged-agent-proof", help="Artifact directory.")
    parser.add_argument("--query", default=DEFAULT_QUERY, help="Search proof query.")
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
        "--managed-plugin-grounding-convergence",
        action="store_true",
        help="Reach initial full retrieval through managed-plugin grounding activation without explicit repair.",
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
    if args.managed_plugin_grounding_convergence and not args.plugin_root:
        parser.error("--plugin-root is required with --managed-plugin-grounding-convergence")
    if args.managed_plugin_grounding_convergence and not args.native_accelerator_lifecycle:
        parser.error("--managed-plugin-grounding-convergence requires --native-accelerator-lifecycle")
    if args.managed_plugin_grounding_convergence and args.managed_plugin_handoff:
        parser.error("--managed-plugin-grounding-convergence and --managed-plugin-handoff are mutually exclusive")
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
    if args.cleanup_proof_temp_root and (
        args.native_accelerator_lifecycle
        or args.intel_runtime_policy
        or args.managed_plugin_handoff
        or args.managed_plugin_grounding_convergence
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
