#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


STABLE_RELEASE_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SEMVER_PARTS_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)


@dataclass(frozen=True)
class ReleaseDecision:
    should_release: bool
    reason: str


def read_version_bytes(data: bytes) -> str:
    package = tomllib.loads(data.decode("utf-8")).get("package", {})
    return str(package.get("version", ""))


def read_current_version(package_path: Path) -> str:
    return read_version_bytes(package_path.read_bytes())


def read_previous_version(before_sha: str, package_path: str) -> str:
    if not before_sha or re.fullmatch(r"0+", before_sha):
        return ""

    result = subprocess.run(
        ["git", "show", f"{before_sha}:{package_path}"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if result.returncode != 0:
        return ""
    return read_version_bytes(result.stdout)


def remote_tag_exists(tag: str) -> bool:
    result = subprocess.run(
        ["git", "ls-remote", "--exit-code", "--tags", "origin", f"refs/tags/{tag}"],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode == 0:
        return True
    if result.returncode == 2:
        return False
    raise RuntimeError(f"failed to inspect remote tag {tag}: {result.stderr.strip()}")


def github_release_exists(tag: str, repo: str) -> bool:
    result = subprocess.run(
        ["gh", "release", "view", tag, "--repo", repo],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode == 0:
        return True

    stderr = result.stderr.lower()
    if "not found" in stderr or "404" in stderr:
        return False
    raise RuntimeError(f"failed to inspect GitHub release {tag}: {result.stderr.strip()}")


def compare_identifiers(left: str, right: str) -> int:
    left_numeric = left.isdigit()
    right_numeric = right.isdigit()
    if left_numeric and right_numeric:
        return (int(left) > int(right)) - (int(left) < int(right))
    if left_numeric != right_numeric:
        return -1 if left_numeric else 1
    return (left > right) - (left < right)


def compare_prerelease(left: str | None, right: str | None) -> int:
    if left == right:
        return 0
    if left is None:
        return 1
    if right is None:
        return -1

    left_parts = left.split(".")
    right_parts = right.split(".")
    for index in range(min(len(left_parts), len(right_parts))):
        compared = compare_identifiers(left_parts[index], right_parts[index])
        if compared:
            return compared
    return (len(left_parts) > len(right_parts)) - (len(left_parts) < len(right_parts))


def compare_semver(left: str, right: str) -> int:
    left_match = SEMVER_PARTS_RE.fullmatch(left)
    right_match = SEMVER_PARTS_RE.fullmatch(right)
    if left_match is None or right_match is None:
        raise ValueError(f"cannot compare non-semver versions: {left!r}, {right!r}.")

    left_core = tuple(int(part) for part in left_match.group(1, 2, 3))
    right_core = tuple(int(part) for part in right_match.group(1, 2, 3))
    if left_core != right_core:
        return (left_core > right_core) - (left_core < right_core)
    return compare_prerelease(left_match.group(4), right_match.group(4))


def decide_release(
    *,
    old_version: str,
    new_version: str,
    tag_exists: bool,
    release_exists: bool,
) -> ReleaseDecision:
    if not STABLE_RELEASE_RE.fullmatch(new_version):
        raise ValueError(
            "codestory-cli version must be a stable release version like X.Y.Z, "
            f"got {new_version!r}."
        )

    if tag_exists != release_exists:
        raise ValueError(
            f"v{new_version} has partial release state "
            f"(tag_exists={tag_exists}, release_exists={release_exists}); refusing automatic retry."
        )

    if tag_exists and release_exists:
        if old_version != new_version:
            raise ValueError(
                f"v{new_version} already has a tag or release; refusing to overwrite it."
            )
        return ReleaseDecision(False, f"v{new_version} already has a tag or release.")

    if old_version == new_version:
        return ReleaseDecision(
            True,
            f"v{new_version} has no tag or release; retrying the current source version.",
        )

    if old_version and compare_semver(new_version, old_version) <= 0:
        raise ValueError(
            f"codestory-cli version moved from {old_version} to {new_version}; "
            "auto-release requires a higher version."
        )

    return ReleaseDecision(True, f"codestory-cli version changed to {new_version}.")


def read_plugin_version(path: Path) -> str:
    manifest = json.loads(path.read_text(encoding="utf-8"))
    version = manifest.get("version")
    if not isinstance(version, str) or not version:
        raise ValueError(f"{path} must declare a string version")
    return version


def read_previous_plugin_version(before_sha: str, manifest_path: str) -> str:
    if not before_sha or set(before_sha) == {"0"}:
        return ""
    try:
        shown = subprocess.run(
            ["git", "show", f"{before_sha}:{manifest_path}"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except subprocess.CalledProcessError:
        return ""
    version = json.loads(shown).get("version")
    return version if isinstance(version, str) else ""


def classify_release_lane(*, cli_version: str, plugin_version: str) -> str:
    """Which release lane a head describes.

    Equal versions release through the native chain. A plugin version ahead of the CLI is
    the plugin fast lane: the plugin ships alone and runs the already-published CLI named by
    its pin. A plugin behind the CLI is always a synchronization error.
    """
    for name, version in (("codestory-cli", cli_version), ("plugin", plugin_version)):
        if not STABLE_RELEASE_RE.fullmatch(version):
            raise ValueError(
                f"{name} version must be a stable release version like X.Y.Z, got {version!r}."
            )
    if plugin_version == cli_version:
        return "native"
    if compare_semver(plugin_version, cli_version) > 0:
        return "plugin"
    raise ValueError(
        f"plugin version {plugin_version} is behind codestory-cli {cli_version}; "
        "synchronize them with scripts/bump-version.mjs."
    )


def write_outputs(
    output_path: str,
    *,
    version: str,
    tag: str,
    decision: ReleaseDecision,
    release_lane: str = "native",
) -> None:
    if not output_path:
        return

    with open(output_path, "a", encoding="utf-8") as output:
        output.write(f"version={version}\n")
        output.write(f"tag={tag}\n")
        output.write(f"should_release={str(decision.should_release).lower()}\n")
        output.write(f"release_lane={release_lane}\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="Detect whether CodeStory should auto-release.")
    parser.add_argument("--before-sha", default=os.environ.get("BEFORE_SHA", ""))
    parser.add_argument(
        "--package-path",
        default="crates/codestory-cli/Cargo.toml",
        help="Path to the codestory-cli Cargo manifest.",
    )
    parser.add_argument(
        "--plugin-manifest-path",
        default="plugins/codestory/.codex-plugin/plugin.json",
        help="Path to the plugin manifest that names the published plugin version.",
    )
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--output", default=os.environ.get("GITHUB_OUTPUT", ""))
    args = parser.parse_args()

    try:
        new_version = read_current_version(Path(args.package_path))
        old_version = read_previous_version(args.before_sha, args.package_path)
        plugin_version = read_plugin_version(Path(args.plugin_manifest_path))
        release_lane = classify_release_lane(
            cli_version=new_version, plugin_version=plugin_version
        )
        # The released artifact on the plugin lane is the plugin, so its version and its
        # previous value drive the decision; the CLI version did not move.
        if release_lane == "plugin":
            old_version = read_previous_plugin_version(
                args.before_sha, args.plugin_manifest_path
            )
            new_version = plugin_version
        tag = f"v{new_version}"
        tag_exists = remote_tag_exists(tag)
        release_exists = github_release_exists(tag, args.repo) if args.repo else False
        decision = decide_release(
            old_version=old_version,
            new_version=new_version,
            tag_exists=tag_exists,
            release_exists=release_exists,
        )
    except (OSError, RuntimeError, ValueError) as error:
        print(f"::error::{error}", file=sys.stderr)
        raise SystemExit(1) from error

    print(f"Previous codestory-cli version: {old_version or '<missing>'}")
    print(f"Current codestory-cli version: {new_version}")
    print(f"Release tag exists: {str(tag_exists).lower()}")
    print(f"GitHub release exists: {str(release_exists).lower()}")
    print(f"Release lane: {release_lane}")
    print(f"Auto release decision: {decision.reason}")
    write_outputs(
        args.output, version=new_version, tag=tag, decision=decision, release_lane=release_lane
    )


if __name__ == "__main__":
    main()
