"""Synchronous subprocess capture self-tests."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from .foundation import ProofFailure, require
from .subprocess_control import run

_DIRECT_STDOUT = "direct-child-stdout\n"
_DIRECT_STDERR = "direct-child-stderr\n"
_LARGE_OUTPUT_BYTES = 2 * 1024 * 1024


def _write_script(path: Path, source: str) -> None:
    path.write_text(source, encoding="utf-8")


def _wait_for_path(path: Path, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists():
        require(
            time.monotonic() < deadline,
            f"timed out waiting for subprocess self-test path {path}",
        )
        time.sleep(0.01)


def _run_inherited_descendant_leg() -> None:
    with tempfile.TemporaryDirectory(
        prefix="codestory-process-capture-descendant-"
    ) as raw:
        root = Path(raw)
        descendant_path = root / "descendant.py"
        parent_path = root / "parent.py"
        ready_path = root / "ready"
        release_path = root / "release"
        stopped_path = root / "stopped"
        _write_script(
            descendant_path,
            """
import sys
import time
from pathlib import Path

ready = Path(sys.argv[1])
release = Path(sys.argv[2])
stopped = Path(sys.argv[3])
ready.write_text("ready\\n", encoding="utf-8")
deadline = time.monotonic() + 2.5
while not release.exists() and time.monotonic() < deadline:
    time.sleep(0.01)
stopped.write_text("stopped\\n", encoding="utf-8")
""".lstrip(),
        )
        _write_script(
            parent_path,
            f"""
import subprocess
import sys
import time
from pathlib import Path

ready = Path(sys.argv[2])
subprocess.Popen([sys.executable, sys.argv[1], *sys.argv[2:]])
deadline = time.monotonic() + 2
while not ready.exists():
    if time.monotonic() >= deadline:
        raise SystemExit("descendant did not start")
    time.sleep(0.01)
sys.stdout.write({_DIRECT_STDOUT!r})
sys.stdout.flush()
sys.stderr.write({_DIRECT_STDERR!r})
sys.stderr.flush()
""".lstrip(),
        )
        command = [
            sys.executable,
            str(parent_path),
            str(descendant_path),
            str(ready_path),
            str(release_path),
            str(stopped_path),
        ]
        started = time.perf_counter()
        result = run(
            command,
            env=os.environ.copy(),
            cwd=root,
            timeout=10,
        )
        elapsed = time.perf_counter() - started
        release_path.write_text("release\n", encoding="utf-8")
        _wait_for_path(stopped_path, 2)
        require(
            elapsed < 1.5,
            "synchronous capture waited for an inheriting descendant instead "
            f"of only the direct child ({elapsed:.3f}s)",
        )
        require(
            result["stdout"] == _DIRECT_STDOUT
            and result["stderr"] == _DIRECT_STDERR,
            "inheriting-descendant capture changed direct-child output",
        )


def _run_large_output_leg() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-process-capture-large-") as raw:
        root = Path(raw)
        script_path = root / "large_output.py"
        stdout = "O" * _LARGE_OUTPUT_BYTES + ":stdout-end\n"
        stderr = "E" * _LARGE_OUTPUT_BYTES + ":stderr-end\n"
        _write_script(
            script_path,
            f"""
import sys

sys.stdout.write("O" * {_LARGE_OUTPUT_BYTES} + ":stdout-end\\n")
sys.stdout.flush()
sys.stderr.write("E" * {_LARGE_OUTPUT_BYTES} + ":stderr-end\\n")
sys.stderr.flush()
""".lstrip(),
        )
        command = [sys.executable, str(script_path)]
        result = run(
            command,
            env=os.environ.copy(),
            cwd=root,
            timeout=10,
        )
        require(
            set(result) == {"command", "exit_code", "wall_ms", "stdout", "stderr"}
            and result["command"] == command
            and result["exit_code"] == 0
            and isinstance(result["wall_ms"], float),
            "file-backed capture changed the synchronous command result shape",
        )
        require(
            result["stdout"] == stdout and result["stderr"] == stderr,
            "file-backed capture truncated output larger than pipe capacity",
        )


def _run_nonzero_tail_leg() -> None:
    with tempfile.TemporaryDirectory(
        prefix="codestory-process-capture-nonzero-"
    ) as raw:
        root = Path(raw)
        script_path = root / "nonzero.py"
        _write_script(
            script_path,
            """
import sys

sys.stdout.write("discarded-stdout-" + "o" * 2500 + "-stdout-tail-sentinel\\n")
sys.stdout.flush()
sys.stderr.write("discarded-stderr-" + "e" * 2500 + "-stderr-tail-sentinel\\n")
sys.stderr.flush()
raise SystemExit(23)
""".lstrip(),
        )
        command = [sys.executable, str(script_path)]
        try:
            run(
                command,
                env=os.environ.copy(),
                cwd=root,
                timeout=10,
            )
        except ProofFailure as error:
            message = str(error)
            require(
                "command failed (23)" in message
                and "stdout-tail-sentinel" in message
                and "stderr-tail-sentinel" in message,
                "nonzero command failure lost its exit code or an output tail",
            )
        else:
            raise ProofFailure("nonzero command passed synchronous capture self-test")


def _run_timeout_leg() -> None:
    with tempfile.TemporaryDirectory(
        prefix="codestory-process-capture-timeout-"
    ) as raw:
        root = Path(raw)
        script_path = root / "timeout.py"
        _write_script(
            script_path,
            """
import sys
import time

sys.stdout.write("timeout-stdout-sentinel\\n")
sys.stdout.flush()
sys.stderr.write("timeout-stderr-sentinel\\n")
sys.stderr.flush()
time.sleep(10)
""".lstrip(),
        )
        command = [sys.executable, str(script_path)]
        try:
            run(
                command,
                env=os.environ.copy(),
                cwd=root,
                timeout=1,
            )
        except subprocess.TimeoutExpired as error:
            require(
                error.cmd == command
                and error.timeout == 1
                and error.stdout
                == f"timeout-stdout-sentinel{os.linesep}".encode()
                and error.stderr
                == f"timeout-stderr-sentinel{os.linesep}".encode(),
                "file-backed capture changed timeout identity or retained output",
            )
        else:
            raise ProofFailure("timed-out command passed synchronous capture self-test")


def run_process_capture_self_tests() -> None:
    _run_inherited_descendant_leg()
    _run_large_output_leg()
    _run_nonzero_tail_leg()
    _run_timeout_leg()
