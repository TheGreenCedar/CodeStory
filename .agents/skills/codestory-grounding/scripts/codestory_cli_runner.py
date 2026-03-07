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


def build_command(subcommand: str, args: Sequence[str]) -> list[str]:
    return ["cargo", "run", "--quiet", "-p", "codestory-cli", "--", subcommand, *args]


def run(subcommand: str) -> int:
    args = list(sys.argv[1:])
    dry_run = False
    if args and args[0] == "--dry-run":
        dry_run = True
        args = args[1:]

    if "--project" not in args:
        args.extend(["--project", os.getcwd()])

    command = build_command(subcommand, args)
    if dry_run:
        print(_quote_command(command))
        return 0

    try:
        completed = subprocess.run(command, cwd=REPO_ROOT, check=False)
    except FileNotFoundError as exc:
        print(f"failed to launch cargo: {exc}", file=sys.stderr)
        return 1

    return completed.returncode
