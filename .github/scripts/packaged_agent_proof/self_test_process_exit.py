"""Exact process exit and retained evidence self-tests."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from .foundation import SERVER_CONSTANT_SET, ProofFailure, require
from .process_identity import ExactProcessExitWaiter, process_start_identity
from .server_cleanup import (
    _cleanup_environment,
    cleanup_projects,
    native_server_exit_wait_budget,
    native_server_exit_wait_required,
    remaining_native_server_exit_wait_ms,
    retained_final_native_server_exit_evidence,
)


def _target_os() -> str:
    if os.name == "nt":
        return "windows"
    return "macos" if sys.platform == "darwin" else "linux"


def _observed_exit_test(target_os: str) -> None:
    process = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(0.05)"],
    )
    start_id = process_start_identity(process.pid)
    waiter = ExactProcessExitWaiter(process.pid, start_id, target_os)
    try:
        process.wait(timeout=5)
        evidence = waiter.wait(5_000)
    finally:
        waiter.close()
    require(
        evidence["status"]
        == ("normal_idle_exit" if os.name == "nt" else "observed_exit")
        and evidence["pid"] == process.pid
        and evidence["process_start_id"] == start_id,
        "exact process normal-exit wait self-test failed",
    )


def _exit_budget_tests() -> dict[str, int]:
    require(
        native_server_exit_wait_required("windows", "installed_runtime")
        and native_server_exit_wait_required("windows", "protected_hardware")
        and not native_server_exit_wait_required("linux", "installed_runtime")
        and not native_server_exit_wait_required("macos", "installed_runtime"),
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


def _retained_exit_tests(budget: dict[str, int]) -> None:
    server_identity = {
        "identity": (123, "windows:504911232000000010"),
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
    _observed_exit_test(target_os)
    _retained_exit_tests(_exit_budget_tests())
    _cleanup_project_tests()
    _cleanup_environment_test()
    _windows_exit_tests(target_os)
    _exit_timeout_test(target_os)
