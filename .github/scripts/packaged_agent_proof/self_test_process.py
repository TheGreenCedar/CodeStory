"""Self Test for packaged CodeStory proof."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from .foundation import ProofFailure, require
from .contracts import (
    assert_retained_json_privacy,
    canonical_sha256,
    load_holdout_task_contracts,
    require_sha256,
    selected_qualification_matrix_cell,
    sha256,
    validate_runtime_claim_scope,
    verify_package_server_contracts,
    write_json,
)
from .archive import (
    embedding_contract_digest,
    expected_archive_digest,
    find_cli,
    load_native_manifest,
    parse_server_proof_identity,
    unpack_archive,
    verify_runtime_against_manifest,
)
from .process import (
    ExactProcessExitWaiter,
    FailurePreservingTemporaryDirectory,
    McpProcess,
    engine_identity,
    extract_resource,
    live_process_executable_sha256,
    native_server_exit_wait_budget,
    native_server_exit_wait_required,
    parse_byte_quantity,
    process_start_identity,
    remaining_native_server_exit_wait_ms,
    require_native_process_start_identity,
    retained_final_native_server_exit_evidence,
    run,
    server_snapshot,
    shared_server_identity,
    verified_live_executable,
)
from .installation import (
    run_parallel,
)
from .runtime import (
    publication_identity_from_status,
    verify_fault_recovery_consistency_raw_evidence,
    verify_publication_fault_raw_evidence,
    verify_retrieval_quality_raw_evidence,
)
from .qualification import (
    derive_scenario_assertions,
    require_candidate_matrix_installation_source,
    verify_retained_qualification,
)
from .calibration import (
    assemble_calibration_bundle,
    build_calibration_self_test_bundle,
    verify_calibration_bundle,
)

def run_process_self_tests() -> None:
    self_target_os = (
        "windows"
        if os.name == "nt"
        else ("macos" if sys.platform == "darwin" else "linux")
    )
    self_pid = os.getpid()
    self_start_id = process_start_identity(self_pid)
    self_live_digest = live_process_executable_sha256(
        self_pid,
        self_start_id,
        self_target_os,
    )
    verified_live_executable(
        pid=self_pid,
        process_start_id=self_start_id,
        reported_sha256=self_live_digest,
        expected_sha256=self_live_digest,
        target_os=self_target_os,
        label="self-test process",
    )
    hostile_reported_digest = (
        ("a" if self_live_digest[0] != "a" else "b") + self_live_digest[1:]
    )
    try:
        verified_live_executable(
            pid=self_pid,
            process_start_id=self_start_id,
            reported_sha256=hostile_reported_digest,
            expected_sha256=self_live_digest,
            target_os=self_target_os,
            label="hostile self-test process",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("self-reported process executable digest bypassed live image hashing")
    stale_start_id = self_start_id[:-1] + (
        "0" if self_start_id[-1] != "0" else "1"
    )
    try:
        live_process_executable_sha256(
            self_pid,
            stale_start_id,
            self_target_os,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("stale process start identity bypassed live image hashing")

    exit_process = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(0.05)"],
    )
    exit_process_start = process_start_identity(exit_process.pid)
    exit_waiter = ExactProcessExitWaiter(
        exit_process.pid,
        exit_process_start,
        self_target_os,
    )
    try:
        exit_process.wait(timeout=5)
        exit_evidence = exit_waiter.wait(5_000)
    finally:
        exit_waiter.close()
    require(
        exit_evidence["status"]
        == ("normal_idle_exit" if os.name == "nt" else "observed_exit")
        and exit_evidence["pid"] == exit_process.pid
        and exit_evidence["process_start_id"] == exit_process_start,
        "exact process normal-exit wait self-test failed",
    )
    require(
        native_server_exit_wait_required("windows", "installed_runtime")
        and native_server_exit_wait_required("windows", "protected_hardware")
        and not native_server_exit_wait_required("linux", "installed_runtime")
        and not native_server_exit_wait_required("macos", "installed_runtime"),
        "native server exit-wait tier selection self-test failed",
    )
    exit_wait_budget = native_server_exit_wait_budget(
        {"server_proof": {"idle_timeout_ms": 60_000}}
    )
    require(
        exit_wait_budget
        == {
            "product_idle_timeout_ms": 60_000,
            "native_teardown_grace_ms": 60_000,
            "timeout_ms": 120_000,
        },
        "native server exit-wait budget self-test failed",
    )
    require(
        remaining_native_server_exit_wait_ms(120.0, 120_000, now=0.0)
        == 120_000
        and remaining_native_server_exit_wait_ms(120.0, 120_000, now=60.0)
        == 60_000,
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
    retained_exit = retained_final_native_server_exit_evidence(
        {
            "status": "normal_idle_exit",
            "pid": 123,
            "process_start_id": "windows:504911232000000010",
            "exit_code": 0,
            "clean_exit_required": True,
            "timeout_ms": 120_000,
        },
        {
            "identity": (123, "windows:504911232000000010"),
            "server_instance_id": "self-test-server",
            "executable_sha256": "a" * 64,
        },
        exit_wait_budget,
        authenticated_process_count=2,
        superseded_process_count=1,
    )
    require(
        retained_exit["pid"] == 123
        and retained_exit["process_start_id"] == "windows:504911232000000010"
        and retained_exit["executable_sha256"] == "a" * 64
        and retained_exit["exit_code"] == 0
        and retained_exit["product_idle_timeout_ms"] == 60_000
        and retained_exit["native_teardown_grace_ms"] == 60_000
        and retained_exit["process_wait_timeout_ms"] == 120_000
        and retained_exit["timeout_ms"] == 120_000
        and retained_exit["authenticated_process_count"] == 2
        and retained_exit["superseded_process_count"] == 1,
        "retained final native server exit evidence self-test failed",
    )
    hostile_exit = dict(retained_exit)
    hostile_exit["exit_code"] = 1
    try:
        retained_final_native_server_exit_evidence(
            hostile_exit,
            {
                "identity": (123, "windows:504911232000000010"),
                "server_instance_id": "self-test-server",
                "executable_sha256": "a" * 64,
            },
            exit_wait_budget,
            authenticated_process_count=2,
            superseded_process_count=1,
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("abnormal final server exit passed cleanup self-test")
    if os.name == "nt":
        with tempfile.TemporaryDirectory(
            prefix="codestory-executable-cleanup-self-test-"
        ) as cleanup_raw:
            cleanup_root = Path(cleanup_raw)
            cleanup_executable = cleanup_root / "proof-process.exe"
            shutil.copy2(
                Path(os.environ["SystemRoot"]) / "System32" / "ping.exe",
                cleanup_executable,
            )
            cleanup_process = subprocess.Popen(
                [str(cleanup_executable), "-n", "2", "127.0.0.1"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            cleanup_start = process_start_identity(cleanup_process.pid)
            cleanup_waiter = ExactProcessExitWaiter(
                cleanup_process.pid,
                cleanup_start,
                self_target_os,
            )
            try:
                cleanup_waiter.wait(5_000)
            finally:
                cleanup_waiter.close()
                cleanup_process.wait(timeout=5)
        require(
            not cleanup_root.exists(),
            "exact process exit wait left its Windows executable locked",
        )
        abnormal_process = subprocess.Popen(
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
        abnormal_start = process_start_identity(abnormal_process.pid)
        abnormal_waiter = ExactProcessExitWaiter(
            abnormal_process.pid,
            abnormal_start,
            self_target_os,
        )
        try:
            abnormal_process.terminate()
            abnormal_process.wait(timeout=5)
            try:
                abnormal_waiter.wait(5_000)
            except ProofFailure as error:
                require(
                    "exited abnormally with code" in str(error),
                    f"abnormal process exit returned the wrong failure: {error}",
                )
                superseded_evidence = abnormal_waiter.wait(
                    5_000,
                    require_clean_exit=False,
                )
                require(
                    superseded_evidence["status"] == "superseded_process_exit"
                    and superseded_evidence["exit_code"] != 0
                    and superseded_evidence["clean_exit_required"] is False,
                    "superseded process exit lost its explicit non-clean status",
                )
            else:
                raise ProofFailure("abnormal process exit passed the exact exit wait")
        finally:
            abnormal_waiter.close()
            if abnormal_process.poll() is None:
                abnormal_process.kill()
                abnormal_process.wait(timeout=5)
    timeout_waiter = ExactProcessExitWaiter(
        self_pid,
        self_start_id,
        self_target_os,
    )
    try:
        try:
            timeout_waiter.wait(1)
        except ProofFailure as error:
            require(
                "did not exit within 1ms" in str(error),
                "exact process exit timeout reported the wrong failure",
            )
        else:
            raise ProofFailure("live process bypassed the bounded exit wait")
    finally:
        timeout_waiter.close()

    preserving_directory = FailurePreservingTemporaryDirectory(
        prefix="codestory-cleanup-error-self-test-"
    )
    preserving_path = Path(preserving_directory.name)
    original_cleanup = preserving_directory.cleanup

    def fail_temporary_cleanup() -> None:
        raise OSError("synthetic temporary cleanup failure")

    preserving_directory.cleanup = fail_temporary_cleanup
    try:
        try:
            with preserving_directory:
                raise ProofFailure("synthetic primary proof failure")
        except ProofFailure as error:
            require(
                str(error).startswith("synthetic primary proof failure")
                and any(
                    "synthetic temporary cleanup failure" in note
                    for note in getattr(error, "__notes__", [])
                ),
                "temporary cleanup error replaced the primary proof failure",
            )
        else:
            raise ProofFailure("temporary cleanup self-test lost the primary failure")
    finally:
        preserving_directory.cleanup = original_cleanup
        original_cleanup()
    require(
        not preserving_path.exists(),
        "temporary cleanup preservation self-test leaked its directory",
    )

    retrying_directory = FailurePreservingTemporaryDirectory(
        prefix="codestory-cleanup-retry-self-test-",
        cleanup_retry_budget_secs=1,
        cleanup_retry_interval_secs=0,
    )
    retrying_path = Path(retrying_directory.name)
    original_retrying_cleanup = retrying_directory.cleanup
    retrying_attempts = 0

    def transient_temporary_cleanup() -> None:
        nonlocal retrying_attempts
        retrying_attempts += 1
        if retrying_attempts < 3:
            raise PermissionError("synthetic transient executable lock")
        original_retrying_cleanup()

    retrying_directory.cleanup = transient_temporary_cleanup
    try:
        with retrying_directory:
            pass
    finally:
        retrying_directory.cleanup = original_retrying_cleanup
        if retrying_path.exists():
            original_retrying_cleanup()
    require(
        retrying_attempts == 3 and not retrying_path.exists(),
        "temporary cleanup retry did not clear a transient executable lock",
    )
