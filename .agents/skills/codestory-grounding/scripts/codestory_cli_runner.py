#!/usr/bin/env python3
"""Run a codestory-cli subcommand from the repo root."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[4]


def _quote_command(command: Sequence[str]) -> str:
    if os.name == "nt":
        return subprocess.list2cmdline(list(command))

    import shlex

    return shlex.join(command)


def _binary_path() -> Path:
    name = "codestory-cli.exe" if os.name == "nt" else "codestory-cli"
    return REPO_ROOT / "target" / "debug" / name


def _build_command() -> list[str]:
    return ["cargo", "build", "-p", "codestory-cli"]


def build_command(subcommand: str, args: Sequence[str]) -> list[str]:
    return [str(_binary_path()), subcommand, *args]


def fallback_command(subcommand: str, args: Sequence[str]) -> list[str]:
    return ["cargo", "run", "--quiet", "-p", "codestory-cli", "--", subcommand, *args]


def run(subcommand: str) -> int:
    args = list(sys.argv[1:])
    dry_run = False
    if args and args[0] == "--dry-run":
        dry_run = True
        args = args[1:]

    if "--project" not in args:
        args.extend(["--project", os.getcwd()])

    if dry_run:
        binary = _binary_path()
        if binary.exists():
            print(_quote_command(build_command(subcommand, args)))
        else:
            print(_quote_command(_build_command()))
            print(_quote_command(build_command(subcommand, args)))
        return 0

    binary = _binary_path()
    if not binary.exists():
        try:
            built = subprocess.run(_build_command(), cwd=REPO_ROOT, check=False)
        except FileNotFoundError as exc:
            print(f"failed to launch cargo: {exc}", file=sys.stderr)
            return 1
        if built.returncode != 0 and not binary.exists():
            command = fallback_command(subcommand, args)
        else:
            command = build_command(subcommand, args)
    else:
        command = build_command(subcommand, args)

    try:
        completed = subprocess.run(command, cwd=REPO_ROOT, check=False)
    except FileNotFoundError as exc:
        if command != fallback_command(subcommand, args):
            fallback = fallback_command(subcommand, args)
            try:
                completed = subprocess.run(fallback, cwd=REPO_ROOT, check=False)
            except FileNotFoundError:
                print(f"failed to launch codestory-cli or cargo: {exc}", file=sys.stderr)
                return 1
            return completed.returncode
        print(f"failed to launch cargo: {exc}", file=sys.stderr)
        return 1

    return completed.returncode
