#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
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


def decide_release(
    *,
    old_version: str,
    new_version: str,
    tag_exists: bool,
    release_exists: bool,
) -> ReleaseDecision:
    if not SEMVER_RE.fullmatch(new_version):
        raise ValueError(f"codestory-cli version must be strict semver, got {new_version!r}.")

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

    return ReleaseDecision(True, f"codestory-cli version changed to {new_version}.")


def write_outputs(output_path: str, *, version: str, tag: str, decision: ReleaseDecision) -> None:
    if not output_path:
        return

    with open(output_path, "a", encoding="utf-8") as output:
        output.write(f"version={version}\n")
        output.write(f"tag={tag}\n")
        output.write(f"should_release={str(decision.should_release).lower()}\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="Detect whether CodeStory should auto-release.")
    parser.add_argument("--before-sha", default=os.environ.get("BEFORE_SHA", ""))
    parser.add_argument(
        "--package-path",
        default="crates/codestory-cli/Cargo.toml",
        help="Path to the codestory-cli Cargo manifest.",
    )
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--output", default=os.environ.get("GITHUB_OUTPUT", ""))
    args = parser.parse_args()

    try:
        new_version = read_current_version(Path(args.package_path))
        old_version = read_previous_version(args.before_sha, args.package_path)
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
    print(f"Auto release decision: {decision.reason}")
    write_outputs(args.output, version=new_version, tag=tag, decision=decision)


if __name__ == "__main__":
    main()
