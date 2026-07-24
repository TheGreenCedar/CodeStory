"""Failure-preserving temporary directory self-tests."""

from __future__ import annotations

from pathlib import Path

from .foundation import ProofFailure, require
from .subprocess_control import FailurePreservingTemporaryDirectory


def _primary_failure_preservation_test() -> None:
    directory = FailurePreservingTemporaryDirectory(
        prefix="codestory-cleanup-error-self-test-"
    )
    path = Path(directory.name)
    original_cleanup = directory.cleanup

    def fail_temporary_cleanup() -> None:
        raise OSError("synthetic temporary cleanup failure")

    directory.cleanup = fail_temporary_cleanup
    try:
        try:
            with directory:
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
        directory.cleanup = original_cleanup
        original_cleanup()
    require(
        not path.exists(),
        "temporary cleanup preservation self-test leaked its directory",
    )


def _transient_cleanup_retry_test() -> None:
    directory = FailurePreservingTemporaryDirectory(
        prefix="codestory-cleanup-retry-self-test-",
        cleanup_retry_budget_secs=1,
        cleanup_retry_interval_secs=0,
    )
    path = Path(directory.name)
    original_cleanup = directory.cleanup
    attempts = 0

    def transient_temporary_cleanup() -> None:
        nonlocal attempts
        attempts += 1
        if attempts < 3:
            raise PermissionError("synthetic transient executable lock")
        original_cleanup()

    directory.cleanup = transient_temporary_cleanup
    try:
        with directory:
            pass
    finally:
        directory.cleanup = original_cleanup
        if path.exists():
            original_cleanup()
    require(
        attempts == 3 and not path.exists(),
        "temporary cleanup retry did not clear a transient executable lock",
    )


def run_process_cleanup_self_tests() -> None:
    _primary_failure_preservation_test()
    _transient_cleanup_retry_test()
