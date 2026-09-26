"""Live process identity proof self-tests."""

from __future__ import annotations

import os
import subprocess
import sys
import time
from unittest.mock import patch

from . import process_identity

from .foundation import ProofFailure
from .process_identity import (
    live_process_executable_sha256,
    process_start_identity,
    verified_live_executable,
)


def _target_os() -> str:
    if os.name == "nt":
        return "windows"
    return "macos" if sys.platform == "darwin" else "linux"


def run_process_identity_self_tests() -> None:
    _macos_terminal_observation_tests()
    if sys.platform == "darwin":
        _macos_terminal_lifecycle_test()
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


def _macos_terminal_observation_tests() -> None:
    """Exercise native result boundaries without launching a process."""
    import ctypes

    prefix_type = process_identity._MacosKinfoProcPrefix
    record = prefix_type()
    record.start.time.seconds = 123
    record.start.time.microseconds = 456
    record.pid = 41732
    record.state = 5
    prefix_size = ctypes.sizeof(record)

    class Function:
        def __init__(self, callback):
            self.callback = callback

        def __call__(self, *args):
            return self.callback(*args)

    def observe(*, capacity=None, actual=None, query_rc=0, read_rc=0):
        capacity = prefix_size * 6 if capacity is None else capacity
        actual = prefix_size if actual is None else actual
        calls = []

        def sysctl(mib, count, output, length, new, new_size):
            assert list(mib) == [1, 14, 1, 41732] and count == 4
            assert new is None and new_size == 0
            calls.append(output is not None)
            size = ctypes.cast(length, ctypes.POINTER(ctypes.c_size_t))
            if output is None:
                size[0] = capacity
                return query_rc
            assert size[0] == capacity
            ctypes.memmove(output, ctypes.byref(record), min(capacity, prefix_size))
            size[0] = actual
            return read_rc

        library = type("Library", (), {})()
        library.sysctl = Function(sysctl)
        with (
            patch.object(process_identity.sys, "platform", "darwin"),
            patch.object(process_identity, "_macos_terminal_layout_supported", return_value=True),
            patch.object(process_identity.ctypes, "CDLL", return_value=library),
        ):
            value = process_identity.macos_terminal_process_observation(41732)
        return value, calls

    observation, calls = observe()
    assert observation.pid == 41732
    assert observation.process_start_id == "macos-proc:123:456"
    assert observation.terminal_state == "zombie" and calls == [False, True]
    # Size queries are capacity estimates, not exact record lengths.
    for kwargs in (
        {"capacity": 0}, {"capacity": prefix_size - 1}, {"capacity": 65537},
        {"actual": 0}, {"actual": prefix_size - 1}, {"actual": prefix_size * 6 + 1},
        {"query_rc": -1}, {"read_rc": -1},
    ):
        assert observe(**kwargs)[0] is None, kwargs
    for field, value in (("pid", 99), ("state", 2), ("state", 0)):
        previous = getattr(record, field)
        setattr(record, field, value)
        assert observe()[0] is None
        setattr(record, field, previous)
    for field, value in (("seconds", 0), ("seconds", -1), ("microseconds", -1), ("microseconds", 1000000)):
        previous = getattr(record.start.time, field)
        setattr(record.start.time, field, value)
        assert observe()[0] is None
        setattr(record.start.time, field, previous)
    with patch.object(process_identity, "_macos_terminal_layout_supported", return_value=False):
        assert process_identity.macos_terminal_process_observation(41732) is None
    with (
        patch.object(process_identity, "_macos_terminal_layout_supported", return_value=True),
        patch.object(process_identity.ctypes, "CDLL", side_effect=OSError("denied")),
    ):
        assert process_identity.macos_terminal_process_observation(41732) is None

    # The existing waiter must not treat a terminal replacement B as evidence
    # that the pinned process A was the process observed in that terminal record.
    waiter = object.__new__(process_identity.ExactProcessExitWaiter)
    waiter.pid = 41732
    waiter.expected_start_id = "macos-proc:123:456"
    with (
        patch.object(process_identity.sys, "platform", "darwin"),
        patch.object(process_identity, "terminated_process_state", return_value=None),
        patch.object(process_identity, "process_start_identity", side_effect=ProofFailure("unreadable")),
        patch.object(process_identity.os, "kill", return_value=None),
        patch.object(process_identity, "macos_terminal_process_observation", return_value=observation),
    ):
        assert waiter._classify_unix_identity()[0] is process_identity._UnixExitProbeState.GONE_OR_REUSED
        waiter.expected_start_id = "macos-proc:123:455"
        assert waiter._classify_unix_identity()[0] is process_identity._UnixExitProbeState.UNKNOWN


def _macos_terminal_lifecycle_test() -> None:
    child = subprocess.Popen(
        [sys.executable, "-B", "-c",
         "import sys; print('ready', flush=True); sys.stdin.buffer.read(1)"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    try:
        assert child.stdout.readline() == b"ready\n"
        identity = process_start_identity(child.pid)
        assert process_identity.macos_terminal_process_observation(child.pid) is None
        child.stdin.close()
        deadline = time.monotonic() + 5
        terminal = None
        # Do not poll/wait the child here: that would reap the retained record.
        while time.monotonic() < deadline:
            terminal = process_identity.macos_terminal_process_observation(child.pid)
            if terminal is not None:
                break
            time.sleep(0.01)
        assert terminal is not None and terminal.process_start_id == identity
        try:
            process_start_identity(child.pid)
        except ProofFailure:
            pass
        else:
            raise ProofFailure("ordinary birth reader admitted a terminal macOS process")
        with patch.object(process_identity, "terminated_process_state", return_value=None):
            waiter = process_identity.ExactProcessExitWaiter(
                child.pid, identity, "macos", allow_already_exited=True,
            )
            assert waiter.exited()
            waiter.close()
        assert child.wait(timeout=5) == 0
        assert process_identity.macos_terminal_process_observation(child.pid) is None
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)
        for stream in (child.stdin, child.stdout, child.stderr):
            if stream is not None and not stream.closed:
                stream.close()
