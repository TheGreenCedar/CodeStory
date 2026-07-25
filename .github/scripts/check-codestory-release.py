#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path


SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)

PLUGIN_MANIFESTS = (
    Path("plugins/codestory/.codex-plugin/plugin.json"),
    Path("plugins/codestory/.claude-plugin/plugin.json"),
    Path("plugins/codestory/.github/plugin/plugin.json"),
)
MODEL_CONTRACT = Path("crates/codestory-llama-sys/model-contract.json")
CLI_VERSION_PIN = Path("plugins/codestory/cli-version.json")
PINNED_CLI_TARGETS = ("macos-arm64", "windows-x64", "linux-x64")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def read_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def workspace_members(root: Path) -> list[Path]:
    manifest = read_toml(root / "Cargo.toml")
    members = manifest.get("workspace", {}).get("members", [])
    return [root / member / "Cargo.toml" for member in members]


def package_info(manifest_path: Path) -> tuple[str, str]:
    manifest = read_toml(manifest_path)
    package = manifest.get("package")
    if not package:
        raise ValueError(f"{manifest_path} does not contain a [package] section")
    name = package.get("name")
    version = package.get("version")
    if not name or not version:
        raise ValueError(f"{manifest_path} must declare package.name and package.version")
    return name, version


def lock_packages(root: Path) -> dict[str, set[str]]:
    lock = read_toml(root / "Cargo.lock")
    packages: dict[str, set[str]] = {}
    for package in lock.get("package", []):
        name = package.get("name")
        version = package.get("version")
        if name and version and name.startswith("codestory-"):
            packages.setdefault(name, set()).add(version)
    return packages


def plugin_versions(root: Path) -> dict[Path, str]:
    versions: dict[Path, str] = {}
    for relative_path in PLUGIN_MANIFESTS:
        manifest_path = root / relative_path
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except FileNotFoundError as exc:
            raise ValueError(f"{manifest_path} does not exist") from exc
        version = manifest.get("version")
        if not isinstance(version, str) or not version:
            raise ValueError(f"{manifest_path} must declare a string version")
        versions[relative_path] = version
    return versions


def validate_model_producer(root: Path, expected: str) -> None:
    contract_path = root / MODEL_CONTRACT
    try:
        contract = json.loads(contract_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"{contract_path} does not exist") from exc
    producer = contract.get("producer")
    if not isinstance(producer, dict):
        raise ValueError(f"{contract_path} must declare a producer object")
    name = producer.get("name")
    version = producer.get("version")
    if name != "codestory-llama-sys":
        raise ValueError(
            f"{MODEL_CONTRACT} producer.name is {name!r}, expected 'codestory-llama-sys'"
        )
    if version != expected:
        raise ValueError(
            f"{MODEL_CONTRACT} producer.version is {version!r}, expected {expected}"
        )


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def validate_cli_version_pin(root: Path, expected: str) -> None:
    """The pin names the CLI the shipped plugin downloads.

    A native release publishes new archives, so the pin must name the version being
    released; its archive digests cannot exist yet and are therefore optional here.
    The plugin release lane owns the diverged case (plugin version ahead of the pin
    with mandatory digests) and carries its own checks.
    """
    pin_path = root / CLI_VERSION_PIN
    try:
        pin = json.loads(pin_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"{pin_path} does not exist") from exc
    except json.JSONDecodeError as exc:
        raise ValueError(f"{pin_path} is not valid JSON: {exc}") from exc
    if pin.get("schema_version") != 1:
        raise ValueError(f"{pin_path} must declare schema_version 1")
    cli_version = pin.get("cli_version")
    if cli_version != expected:
        raise ValueError(
            f"{pin_path} cli_version is {cli_version!r}, expected {expected}; "
            "bump it with scripts/bump-version.mjs"
        )
    if pin.get("release_tag") != f"v{expected}":
        raise ValueError(f"{pin_path} release_tag must be v{expected}")
    archives = pin.get("archives")
    if archives is not None:
        if not isinstance(archives, dict):
            raise ValueError(f"{pin_path} archives must be an object when present")
        for target, digest in archives.items():
            if target not in PINNED_CLI_TARGETS:
                raise ValueError(f"{pin_path} archives names unknown target {target!r}")
            if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
                raise ValueError(f"{pin_path} archives[{target!r}] must be a lowercase sha256")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate synchronized CodeStory release version surfaces.",
    )
    parser.add_argument("--version", required=True, help="Expected release version, without v prefix.")
    parser.add_argument(
        "--project-root",
        default=".",
        help="Repository root containing Cargo.toml and Cargo.lock.",
    )
    args = parser.parse_args()

    expected = args.version.removeprefix("v")
    if not SEMVER_RE.fullmatch(expected):
        fail(f"version must be strict semver like 0.7.0, got {args.version!r}")

    root = Path(args.project_root).resolve()
    cli_manifest = root / "crates" / "codestory-cli" / "Cargo.toml"
    cli_name, cli_version = package_info(cli_manifest)
    if cli_name != "codestory-cli":
        fail(f"{cli_manifest} package.name is {cli_name!r}, expected 'codestory-cli'")
    if cli_version != expected:
        fail(f"codestory-cli version is {cli_version}, expected {expected}")

    try:
        validate_model_producer(root, expected)
    except ValueError as exc:
        fail(str(exc))

    for manifest_path, current_plugin_version in plugin_versions(root).items():
        if current_plugin_version != expected:
            fail(f"{manifest_path} version is {current_plugin_version}, expected {expected}")

    try:
        validate_cli_version_pin(root, expected)
    except ValueError as exc:
        fail(str(exc))

    workspace_versions: dict[str, str] = {}
    for manifest_path in workspace_members(root):
        name, version = package_info(manifest_path)
        if not name.startswith("codestory-"):
            continue
        workspace_versions[name] = version
        if version != expected:
            fail(f"{manifest_path.relative_to(root)} is {version}, expected {expected}")

    if "codestory-cli" not in workspace_versions:
        fail("workspace members do not include codestory-cli")

    lock_versions = lock_packages(root)
    for name in sorted(workspace_versions):
        versions = lock_versions.get(name)
        if not versions:
            fail(f"Cargo.lock does not contain package entry for {name}")
        if versions != {expected}:
            fail(f"Cargo.lock package {name} versions are {sorted(versions)}, expected {expected}")

    extra_lock_mismatches = {
        name: versions
        for name, versions in lock_versions.items()
        if name.startswith("codestory-") and versions != {expected}
    }
    if extra_lock_mismatches:
        details = ", ".join(
            f"{name}={sorted(versions)}" for name, versions in sorted(extra_lock_mismatches.items())
        )
        fail(f"Cargo.lock has CodeStory version mismatches: {details}")

    print(
        f"CodeStory release version {expected} is synchronized across "
        f"{len(workspace_versions)} workspace crates, Cargo.lock, "
        f"{len(PLUGIN_MANIFESTS)} codestory plugin manifests, and the CLI version "
        "pin; the embedded-model producer matches."
    )


if __name__ == "__main__":
    main()
