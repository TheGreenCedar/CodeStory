"""Live process identity proof self-tests."""

from __future__ import annotations

import os
import sys

from .foundation import ProofFailure
from .process import (
    live_process_executable_sha256,
    process_start_identity,
    verified_live_executable,
)


def _target_os() -> str:
    if os.name == "nt":
        return "windows"
    return "macos" if sys.platform == "darwin" else "linux"


def run_process_identity_self_tests() -> None:
    target_os = _target_os()
    pid = os.getpid()
    start_id = process_start_identity(pid)
    live_digest = live_process_executable_sha256(pid, start_id, target_os)
    verified_live_executable(
        pid=pid,
        process_start_id=start_id,
        reported_sha256=live_digest,
        expected_sha256=live_digest,
        target_os=target_os,
        label="self-test process",
    )
    hostile_digest = ("a" if live_digest[0] != "a" else "b") + live_digest[1:]
    try:
        verified_live_executable(
            pid=pid,
            process_start_id=start_id,
            reported_sha256=hostile_digest,
            expected_sha256=live_digest,
            target_os=target_os,
            label="hostile self-test process",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "self-reported process executable digest bypassed live image hashing"
        )
    stale_start_id = start_id[:-1] + ("0" if start_id[-1] != "0" else "1")
    try:
        live_process_executable_sha256(pid, stale_start_id, target_os)
    except ProofFailure:
        pass
    else:
        raise ProofFailure("stale process start identity bypassed live image hashing")
