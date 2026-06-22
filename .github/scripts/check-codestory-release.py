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


def plugin_version(root: Path) -> str:
    manifest_path = root / "plugins" / "codestory" / ".codex-plugin" / "plugin.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"{manifest_path} does not exist") from exc
    version = manifest.get("version")
    if not isinstance(version, str) or not version:
        raise ValueError(f"{manifest_path} must declare a string version")
    return version


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


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

    current_plugin_version = plugin_version(root)
    if current_plugin_version != expected:
        fail(
            "plugins/codestory/.codex-plugin/plugin.json version is "
            f"{current_plugin_version}, expected {expected}"
        )

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
        f"{len(workspace_versions)} workspace crates, Cargo.lock, and the codestory plugin."
    )


if __name__ == "__main__":
    main()
