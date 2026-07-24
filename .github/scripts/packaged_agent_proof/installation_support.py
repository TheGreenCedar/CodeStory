"""Shared installation-proof utilities."""

from __future__ import annotations

import hashlib
import os
import secrets
import threading
from collections.abc import Callable
from pathlib import Path

from .foundation import (
    LEGACY_TOKENS,
    QUALIFICATION_SCHEMA_VERSION,
    ProofFailure,
    require,
)


def directory_contract_sha256(root: Path) -> str:
    require(root.is_dir(), f"plugin package root does not exist: {root}")
    digest = hashlib.sha256()
    files = sorted(path for path in root.rglob("*") if path.is_file())
    require(files, "plugin package root is empty")
    for path in files:
        require(
            not path.is_symlink(),
            f"installed plugin package contains a symlink: {path}",
        )
        relative = path.relative_to(root).as_posix().encode("utf-8")
        payload = path.read_bytes()
        digest.update(len(relative).to_bytes(8, "little"))
        digest.update(relative)
        digest.update(len(payload).to_bytes(8, "little"))
        digest.update(payload)
    return digest.hexdigest()


def same_existing_path(first: Path, second: Path) -> bool:
    return first.exists() and second.exists() and os.path.samefile(first, second)


def run_parallel(tasks: dict[str, Callable[[], object]]) -> dict[str, object]:
    results: dict[str, object] = {}
    failures: list[tuple[str, BaseException]] = []
    lock = threading.Lock()

    def invoke(name: str, task: Callable[[], object]) -> None:
        try:
            value = task()
            with lock:
                results[name] = value
        except BaseException as exc:  # noqa: BLE001 - preserve worker failure for the proof.
            with lock:
                failures.append((name, exc))

    threads = [
        threading.Thread(target=invoke, args=(name, task), daemon=True)
        for name, task in tasks.items()
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    if failures:
        failures.sort(key=lambda item: item[0])
        details = "; ".join(f"{name}: {failure}" for name, failure in failures)
        raise ProofFailure(
            f"parallel qualification tasks failed: {details}"
        ) from failures[0][1]
    return results


def isolated_environment(
    root: Path, policy: str | None, offline: bool
) -> dict[str, str]:
    env = dict(os.environ)
    home = root / "home"
    cache = root / "cache"
    data = root / "plugin-data"
    temp = root / "tmp"
    runtime = root / "runtime"
    for path in (home, cache, data, temp, runtime):
        path.mkdir(parents=True, exist_ok=True)
    runtime.chmod(0o700)
    env.update(
        {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "CODESTORY_CACHE_ROOT": str(cache),
            "CODESTORY_PLUGIN_DATA": str(data),
            "TMPDIR": str(temp),
            "TEMP": str(temp),
            "TMP": str(temp),
            "XDG_RUNTIME_DIR": str(runtime),
            "CODESTORY_EMBED_ALLOW_CPU": "1" if policy == "cpu_explicit" else "0",
        }
    )
    if offline:
        env.update(
            {
                "HTTP_PROXY": "http://127.0.0.1:1",
                "HTTPS_PROXY": "http://127.0.0.1:1",
                "ALL_PROXY": "http://127.0.0.1:1",
                "NO_PROXY": "",
                "CODESTORY_PLUGIN_DISABLE_PROVISION": "1",
            }
        )
    for key in list(env):
        if key.startswith("CODESTORY_EMBED_") and key != "CODESTORY_EMBED_ALLOW_CPU":
            del env[key]
    return env


def qualification_environment(
    root: Path, env: dict[str, str]
) -> tuple[dict[str, str], dict]:
    proof_root = (root / "qualification").resolve()
    proof_root.mkdir(parents=True, exist_ok=True)
    proof_root.chmod(0o700)
    nonce = secrets.token_hex(32)
    qualified = dict(env)
    qualified["CODESTORY_EMBED_QUALIFICATION_DIR"] = str(proof_root)
    qualified["CODESTORY_EMBED_QUALIFICATION_NONCE"] = nonce
    return qualified, {
        "schema_version": QUALIFICATION_SCHEMA_VERSION,
        "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
    }


def assert_no_legacy_state(cache_root: Path) -> None:
    offenders = []
    for path in cache_root.rglob("*"):
        lowered = path.name.lower()
        if (
            any(token in lowered for token in LEGACY_TOKENS)
            or path.suffix.lower() == ".pid"
        ):
            offenders.append(str(path))
    require(
        not offenders, "legacy process state was created: " + ", ".join(offenders[:10])
    )


def create_second_repository(root: Path) -> Path:
    repo = root / "second-repository"
    repo.mkdir()
    (repo / "README.md").write_text(
        "# Second repository\n\nA tiny warm-engine reuse fixture.\n",
        encoding="utf-8",
    )
    (repo / "lib.rs").write_text(
        'pub fn shared_engine_probe() -> &\'static str { "warm" }\n',
        encoding="utf-8",
    )
    return repo
