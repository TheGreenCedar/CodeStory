"""Exact process exit and retained evidence self-tests."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

from .foundation import SERVER_CONSTANT_SET, ProofFailure, require
from .process_identity import (
    ExactProcessExitWaiter,
    _UnixExitProbeState,
    process_start_identity,
)
from .server_cleanup import (
    _cleanup_environment,
    cleanup_projects,
    native_server_exit_wait_budget,
    native_server_exit_wait_required,
    remaining_native_server_exit_wait_ms,
    retained_final_native_server_exit_evidence,
    wait_for_final_temporary_package_server,
)


def _temporary_boundary_ordering_test() -> None:
    source = (Path(__file__).parent / "runtime_contract.py").read_text(encoding="utf-8")
    start = source.index("def run_runtime_proof(")
    end = source.index("\ndef installed_runtime_provenance_is_proven", start)
    body = source[start:end]
    proof = body.index("runtime = prove_runtime(")
    cleanup = body.index("cleanup = wait_for_final_temporary_package_server(")
    returned = body.index("return runtime")
    require(
        proof < cleanup < returned,
        "runtime proof no longer fences the exact native server before"
        " returning to the temporary-directory boundary",
    )


def _target_os() -> str:
    if os.name == "nt":
        return "windows"
    return "macos" if sys.platform == "darwin" else "linux"


def _start_handshaken_child() -> subprocess.Popen:
    process = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import sys; sys.stdout.buffer.write(b'ready\\n');"
            " sys.stdout.buffer.flush(); sys.stdin.buffer.read(1)",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    require(
        process.stdout is not None and process.stdout.readline() == b"ready\n",
        "exact-exit self-test child did not enter its handshaken live state",
    )
    return process


def _release_handshaken_child(process: subprocess.Popen) -> None:
    require(
        process.stdin is not None and not process.stdin.closed,
        "exact-exit self-test child lost its release channel",
    )
    process.stdin.write(b"x")
    process.stdin.close()
    process.wait(timeout=5)


def _close_handshaken_child(process: subprocess.Popen) -> None:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=5)
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None and not stream.closed:
            stream.close()


def _synthetic_unix_waiter() -> ExactProcessExitWaiter:
    waiter = object.__new__(ExactProcessExitWaiter)
    waiter.pid = 41732
    waiter.expected_start_id = "linux:639221697923142260"
    waiter.target_os = "linux"
    waiter.handle = None
    waiter._already_exited_reason = None
    waiter._unix_probe_state = None
    waiter._unix_probe_detail = None
    return waiter


def _unix_exit_deadline_tests() -> None:
    gone = _UnixExitProbeState.GONE_OR_REUSED
    matching = _UnixExitProbeState.MATCHING
    unknown = _UnixExitProbeState.UNKNOWN

    for case, timeout_ms, probe_delay_ms, detail in (
        ("zero-time already gone", 0, 0, "no longer exists"),
        ("slow initial gone", 10, 11, "no longer exists"),
        (
            "slow initial PID reuse",
            10,
            11,
            "now carries start identity linux:2, replacing linux:1",
        ),
    ):
        waiter = _synthetic_unix_waiter()
        clock = {"seconds": 0.0}

        def classify_gone() -> tuple[_UnixExitProbeState, str | None]:
            clock["seconds"] += probe_delay_ms / 1000
            return gone, detail

        waiter._wait_unix(
            timeout_ms,
            now=lambda: clock["seconds"],
            sleep=lambda _duration: None,
            classify=classify_gone,
        )
        require(
            clock["seconds"] == probe_delay_ms / 1000
            and waiter._already_exited_reason == detail,
            f"{case} was rejected",
        )

    waiter = _synthetic_unix_waiter()
    oversleep_clock = {"seconds": 0.0}
    oversleep_probes = {"count": 0}

    def oversleep_live() -> tuple[_UnixExitProbeState, None]:
        oversleep_probes["count"] += 1
        return matching, None

    try:
        waiter._wait_unix(
            10,
            now=lambda: oversleep_clock["seconds"],
            sleep=lambda _duration: oversleep_clock.update(seconds=0.011),
            classify=oversleep_live,
        )
    except ProofFailure as error:
        require(
            oversleep_probes["count"] == 1 and "did not exit within 10ms" in str(error),
            "Unix oversleep started a late second identity probe",
        )
    else:
        raise ProofFailure("Unix oversleep bypassed the exact-exit deadline")

    waiter = _synthetic_unix_waiter()
    late_clock = {"seconds": 0.0}
    late_probes = {"count": 0}

    def late_exit() -> tuple[_UnixExitProbeState, str | None]:
        late_probes["count"] += 1
        if late_probes["count"] == 1:
            return matching, None
        late_clock["seconds"] = 0.010
        return gone, "no longer exists"

    try:
        waiter._wait_unix(
            10,
            now=lambda: late_clock["seconds"],
            sleep=lambda _duration: late_clock.update(seconds=0.001),
            classify=late_exit,
        )
    except ProofFailure as error:
        require(
            late_probes["count"] == 2
            and "exit was first observed only after the wait expired" in str(error),
            "Unix slow later probe returned ambiguous deadline evidence",
        )
    else:
        raise ProofFailure("Unix exit first observed at the deadline passed")

    for case, states in (
        ("initial unknown then gone", [unknown, gone]),
        ("later unknown then gone", [matching, unknown, gone]),
    ):
        waiter = _synthetic_unix_waiter()
        clock = {"seconds": 0.0}
        probes = {"count": 0}

        def transient_unknown() -> tuple[_UnixExitProbeState, str | None]:
            state = states[probes["count"]]
            probes["count"] += 1
            return (
                (state, "no longer exists")
                if state is gone
                else (
                    (state, "synthetic unreadable exact identity")
                    if state is unknown
                    else (state, None)
                )
            )

        waiter._wait_unix(
            10,
            now=lambda: clock["seconds"],
            sleep=lambda _duration: clock.update(
                seconds=clock["seconds"] + 0.001
            ),
            classify=transient_unknown,
        )
        require(
            probes["count"] == len(states)
            and waiter._already_exited_reason == "no longer exists",
            f"Unix {case} did not accept timely proven exit",
        )

    waiter = _synthetic_unix_waiter()
    live_clock = {"seconds": 0.0}
    live_probes = {"count": 0}

    def permanently_live() -> tuple[_UnixExitProbeState, None]:
        live_probes["count"] += 1
        return matching, None

    try:
        waiter._wait_unix(
            10,
            now=lambda: live_clock["seconds"],
            sleep=lambda _duration: live_clock.update(
                seconds=live_clock["seconds"] + 0.005
            ),
            classify=permanently_live,
        )
    except ProofFailure as error:
        require(
            live_probes["count"] == 2
            and "still running with its start identity unchanged" in str(error),
            "Unix matching process returned wrong deadline evidence",
        )
    else:
        raise ProofFailure("Unix live process bypassed the exact-exit deadline")

    waiter = _synthetic_unix_waiter()
    unknown_clock = {"seconds": 0.0}
    unknown_probes = {"count": 0}

    def permanently_unknown() -> tuple[_UnixExitProbeState, str]:
        unknown_probes["count"] += 1
        return unknown, "synthetic unreadable exact identity"

    try:
        waiter._wait_unix(
            10,
            now=lambda: unknown_clock["seconds"],
            sleep=lambda _duration: unknown_clock.update(
                seconds=unknown_clock["seconds"] + 0.005
            ),
            classify=permanently_unknown,
        )
    except ProofFailure as error:
        require(
            unknown_probes["count"] == 2
            and "left its exact identity uncertain" in str(error)
            and "synthetic unreadable exact identity" in str(error)
            and "still running" not in str(error),
            "Unix unknown process state returned wrong deadline evidence",
        )
    else:
        raise ProofFailure("Unix permanently unknown process state was accepted")


def _observed_exit_test(target_os: str) -> None:
    process = _start_handshaken_child()
    waiter = None
    try:
        start_id = process_start_identity(process.pid)
        waiter = ExactProcessExitWaiter(process.pid, start_id, target_os)
        _release_handshaken_child(process)
        evidence = waiter.wait(
            5_000,
            require_clean_exit=target_os == "windows",
        )
    finally:
        if waiter is not None:
            waiter.close()
        _close_handshaken_child(process)
    require(
        evidence["status"]
        == ("normal_idle_exit" if os.name == "nt" else "observed_exit")
        and evidence["pid"] == process.pid
        and evidence["process_start_id"] == start_id
        and evidence["clean_exit_required"] is (target_os == "windows")
        and evidence["exit_code"] == (0 if target_os == "windows" else None),
        "exact process normal-exit wait self-test failed",
    )


def _constructor_unknown_then_exit_test(target_os: str) -> None:
    if target_os == "windows":
        return
    process = _start_handshaken_child()
    waiter = None
    unknown_detail = "synthetic unreadable exact identity"
    try:
        start_id = process_start_identity(process.pid)
        with patch.object(
            ExactProcessExitWaiter,
            "_classify_unix_identity",
            return_value=(_UnixExitProbeState.UNKNOWN, unknown_detail),
        ):
            waiter = ExactProcessExitWaiter(process.pid, start_id, target_os)
            require(
                waiter._unix_probe_state is _UnixExitProbeState.UNKNOWN
                and waiter._unix_probe_detail == unknown_detail,
                "Unix constructor did not preserve its unknown identity state",
            )
            try:
                waiter.exited()
            except ProofFailure as error:
                require(
                    unknown_detail in str(error),
                    "Unix boolean exit probe returned wrong unknown-state failure",
                )
            else:
                raise ProofFailure("Unix boolean exit probe accepted unknown identity")
        _release_handshaken_child(process)
        evidence = waiter.wait(5_000, require_clean_exit=False)
    finally:
        if waiter is not None:
            waiter.close()
        _close_handshaken_child(process)
    require(
        evidence["status"] == "observed_exit"
        and evidence["pid"] == process.pid
        and evidence["process_start_id"] == start_id,
        "Unix waiter did not reclassify constructor-unknown process after exit",
    )


def _exit_budget_tests() -> dict[str, int]:
    require(
        all(
            native_server_exit_wait_required(target_os, proof_tier)
            for target_os in ("linux", "macos", "windows")
            for proof_tier in (
                "calibration",
                "hosted_package",
                "protected_hardware",
                "installed_runtime",
            )
        )
        and not native_server_exit_wait_required("freebsd", "installed_runtime")
        and not native_server_exit_wait_required("macos", "version_only"),
        "native server exit-wait tier selection self-test failed",
    )
    manifest = {"server_proof": {"idle_timeout_ms": 60_000}}
    budget = native_server_exit_wait_budget(
        manifest,
        {
            "status": "frozen",
            "fixed_contract_values": {
                "idle_timeout_ms": 60_000,
                "true_idle_observation_grace_ms": 2_500,
            },
        },
    )
    require(
        budget
        == {
            "product_idle_timeout_ms": 60_000,
            "native_teardown_grace_ms": 60_000,
            "timeout_ms": 120_000,
        },
        "native server exit-wait budget self-test failed",
    )
    require(
        native_server_exit_wait_budget(
            manifest,
            {"status": "draft", "qualification_thresholds": {"true_idle_exit": None}},
        )
        == budget,
        "an unfrozen constant set changed the conservative exit-wait budget",
    )
    for hostile in (
        {"status": "frozen"},
        {"status": "frozen", "fixed_contract_values": {}},
        {
            "status": "frozen",
            "fixed_contract_values": {
                "idle_timeout_ms": 60_000,
                "true_idle_observation_grace_ms": None,
            },
        },
        {
            "status": "frozen",
            "fixed_contract_values": {
                "idle_timeout_ms": 60_000,
                "true_idle_observation_grace_ms": 60_001,
            },
        },
    ):
        try:
            native_server_exit_wait_budget(manifest, hostile)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"a broken frozen constant set produced an exit-wait budget: {hostile!r}"
            )
    checked_in = json.loads(SERVER_CONSTANT_SET.read_text(encoding="utf-8"))
    require(
        native_server_exit_wait_budget(manifest, checked_in) == budget,
        "the checked-in constant set no longer clears the exit-wait floor",
    )
    require(
        remaining_native_server_exit_wait_ms(120.0, 120_000, now=0.0) == 120_000
        and remaining_native_server_exit_wait_ms(120.0, 120_000, now=60.0) == 60_000,
        "native server shared exit-wait deadline self-test failed",
    )
    try:
        remaining_native_server_exit_wait_ms(120.0, 120_000, now=120.0)
    except ProofFailure as error:
        require(
            "shared 120000ms exit-wait bound" in str(error),
            "native server shared exit-wait timeout reported the wrong failure",
        )
    else:
        raise ProofFailure("expired native server shared exit-wait deadline passed")
    return budget


def _cleanup_project_tests() -> None:
    project = str(Path.cwd().resolve())
    require(
        cleanup_projects({"projects": [project]}) == [Path(project)]
        and cleanup_projects({"projects": [project, project]})
        == [Path(project), Path(project)],
        "one- and two-project cleanup contexts were rejected",
    )
    for projects in ([], [project, project, project], ["relative"], [1]):
        try:
            cleanup_projects({"projects": projects})
        except ProofFailure:
            pass
        else:
            raise ProofFailure(f"invalid cleanup projects were accepted: {projects!r}")


def _cleanup_environment_test() -> None:
    with tempfile.TemporaryDirectory(
        prefix="codestory-cleanup-environment-self-test-"
    ) as raw:
        root = Path(raw)
        manifest_path = root / "codestory-native-manifest.json"
        manifest_path.write_text("{}\n", encoding="utf-8")
        cleanup = _cleanup_environment(
            {
                "CODESTORY_CLI": "stale-launcher",
                "CODESTORY_PLUGIN_CLI_MANIFEST_PATH": "stale-manifest",
            },
            {
                "qualification_directory": str(root),
                "qualification_nonce": "self-test-nonce",
                "plugin_cli_archive_sha256": "a" * 64,
                "plugin_cli_manifest_path": str(manifest_path),
            },
        )
        require(
            "CODESTORY_CLI" not in cleanup
            and cleanup["CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"] == "a" * 64
            and cleanup["CODESTORY_PLUGIN_CLI_MANIFEST_PATH"] == str(manifest_path),
            "final cleanup lost the split runtime package environment",
        )


def _no_server_ground_only_cleanup_test(target_os: str) -> None:
    asset_target = {
        "linux": "linux-x64",
        "macos": "macos-arm64",
        "windows": "windows-x64",
    }[target_os]
    control = {"_waiters": []}
    result = wait_for_final_temporary_package_server(
        Namespace(proof_tier="installed_runtime", timeout_secs=1),
        {},
        control,
        {
            "asset_target": asset_target,
            "server_proof": {"idle_timeout_ms": 60_000},
        },
        {
            "status": "unfrozen",
            "qualification_thresholds": {"true_idle_exit": None},
        },
        require_final_server=False,
    )
    require(
        result is None and control["_waiters"] == [],
        "ground-only cleanup invented a required final native server",
    )


def _retained_exit_tests(budget: dict[str, int]) -> None:
    server_identity = {
        "identity": (123, "windows:504911232000000010"),
        "target_os": "windows",
        "server_instance_id": "self-test-server",
        "executable_sha256": "a" * 64,
    }
    retained = retained_final_native_server_exit_evidence(
        {
            "status": "normal_idle_exit",
            "pid": 123,
            "process_start_id": "windows:504911232000000010",
            "exit_code": 0,
            "clean_exit_required": True,
            "timeout_ms": 120_000,
        },
        server_identity,
        budget,
        authenticated_process_count=2,
        superseded_process_count=1,
    )
    require(
        retained["pid"] == 123
        and retained["process_start_id"] == "windows:504911232000000010"
        and retained["executable_sha256"] == "a" * 64
        and retained["exit_code"] == 0
        and retained["product_idle_timeout_ms"] == 60_000
        and retained["native_teardown_grace_ms"] == 60_000
        and retained["process_wait_timeout_ms"] == 120_000
        and retained["timeout_ms"] == 120_000
        and retained["authenticated_process_count"] == 2
        and retained["superseded_process_count"] == 1,
        "retained final native server exit evidence self-test failed",
    )
    hostile_exit = dict(retained)
    hostile_exit["exit_code"] = 1
    try:
        retained_final_native_server_exit_evidence(
            hostile_exit,
            server_identity,
            budget,
            authenticated_process_count=2,
            superseded_process_count=1,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("abnormal final server exit passed cleanup self-test")

    for target_os, process_start_id in (
        ("linux", "linux:123"),
        ("macos", "macos-proc:123:456"),
    ):
        unix_identity = {
            "identity": (123, process_start_id),
            "target_os": target_os,
            "server_instance_id": f"self-test-{target_os}-server",
            "executable_sha256": "b" * 64,
        }
        unix_retained = retained_final_native_server_exit_evidence(
            {
                "status": "observed_exit",
                "pid": 123,
                "process_start_id": process_start_id,
                "exit_code": None,
                "clean_exit_required": False,
                "timeout_ms": 120_000,
            },
            unix_identity,
            budget,
            authenticated_process_count=1,
            superseded_process_count=0,
        )
        require(
            unix_retained["status"] == "observed_exit"
            and unix_retained["exit_code"] is None,
            f"{target_os} retained exit evidence invented a process exit code",
        )
        hostile_unix = dict(unix_retained)
        hostile_unix["status"] = "normal_idle_exit"
        hostile_unix["exit_code"] = 0
        hostile_unix["clean_exit_required"] = True
        try:
            retained_final_native_server_exit_evidence(
                hostile_unix,
                unix_identity,
                budget,
                authenticated_process_count=1,
                superseded_process_count=0,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"{target_os} cleanup accepted an invented clean exit-code receipt"
            )


def _windows_exit_tests(target_os: str) -> None:
    if os.name != "nt":
        return
    with tempfile.TemporaryDirectory(
        prefix="codestory-executable-cleanup-self-test-"
    ) as cleanup_raw:
        cleanup_root = Path(cleanup_raw)
        cleanup_executable = cleanup_root / "proof-process.exe"
        shutil.copy2(
            Path(os.environ["SystemRoot"]) / "System32" / "ping.exe",
            cleanup_executable,
        )
        process = subprocess.Popen(
            [str(cleanup_executable), "-n", "2", "127.0.0.1"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        start_id = process_start_identity(process.pid)
        waiter = ExactProcessExitWaiter(process.pid, start_id, target_os)
        try:
            waiter.wait(5_000)
        finally:
            waiter.close()
            process.wait(timeout=5)
    require(
        not cleanup_root.exists(),
        "exact process exit wait left its Windows executable locked",
    )
    _windows_abnormal_exit_test(target_os)


def _windows_abnormal_exit_test(target_os: str) -> None:
    process = subprocess.Popen(
        [
            str(Path(os.environ["SystemRoot"]) / "System32" / "ping.exe"),
            "-n",
            "30",
            "127.0.0.1",
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    start_id = process_start_identity(process.pid)
    waiter = ExactProcessExitWaiter(process.pid, start_id, target_os)
    try:
        process.terminate()
        process.wait(timeout=5)
        try:
            waiter.wait(5_000)
        except ProofFailure as error:
            require(
                "exited abnormally with code" in str(error),
                f"abnormal process exit returned the wrong failure: {error}",
            )
            evidence = waiter.wait(5_000, require_clean_exit=False)
            require(
                evidence["status"] == "superseded_process_exit"
                and evidence["exit_code"] != 0
                and evidence["clean_exit_required"] is False,
                "superseded process exit lost its explicit non-clean status",
            )
        else:
            raise ProofFailure("abnormal process exit passed the exact exit wait")
    finally:
        waiter.close()
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)


def _exit_timeout_test(target_os: str) -> None:
    pid = os.getpid()
    start_id = process_start_identity(pid)
    waiter = ExactProcessExitWaiter(pid, start_id, target_os)
    try:
        try:
            waiter.wait(1)
        except ProofFailure as error:
            message = str(error)
            require(
                f"exact process {pid}" in message
                and start_id in message
                and "did not exit within 1ms" in message
                and "waited" in message
                and "still running" in message,
                "exact process exit timeout omitted its pid, identity, waited"
                f" duration, or final process state: {message}",
            )
        else:
            raise ProofFailure("live process bypassed the bounded exit wait")
    finally:
        waiter.close()


def run_process_exit_self_tests() -> None:
    target_os = _target_os()
    _unix_exit_deadline_tests()
    _observed_exit_test(target_os)
    _constructor_unknown_then_exit_test(target_os)
    _retained_exit_tests(_exit_budget_tests())
    _cleanup_project_tests()
    _cleanup_environment_test()
    _no_server_ground_only_cleanup_test(target_os)
    _temporary_boundary_ordering_test()
    _windows_exit_tests(target_os)
    _exit_timeout_test(target_os)
