"""Exact-process ownership and final temporary package-server cleanup."""

from __future__ import annotations

import argparse
import math
import time
from pathlib import Path

from .contracts import (
    require_nonempty_string,
    require_positive_int,
    require_sha256,
)
from .foundation import (
    NATIVE_SERVER_TEARDOWN_GRACE_MS,
    TARGET_CONTRACTS,
    ProofFailure,
    require,
)
from .process_identity import (
    ExactProcessExitWaiter,
    require_native_process_start_identity,
    verified_live_executable,
)
from .server_identity import find_value, server_snapshot
from .subprocess_control import McpProcess


def pin_temporary_package_server(
    control: dict,
    server_process: dict,
    manifest: dict,
    target_os: str,
    label: str,
) -> dict:
    pid = require_positive_int(server_process.get("pid"), f"{label} pid")
    process_start_id = require_native_process_start_identity(
        server_process.get("process_start_id"),
        target_os,
        f"{label} process start identity",
    )
    waiters = control.setdefault("_waiters", [])
    require(isinstance(waiters, list), "temporary package cleanup waiter state is invalid")
    for entry in waiters:
        require(isinstance(entry, dict), "temporary package cleanup waiter is malformed")
        if entry.get("identity") == (pid, process_start_id):
            return entry
    verified_process = verified_live_executable(
        pid=pid,
        process_start_id=process_start_id,
        reported_sha256=server_process["executable_sha256"],
        expected_sha256=manifest["binary"]["sha256"],
        target_os=target_os,
        label=label,
    )
    entry = {
        "identity": (pid, process_start_id),
        "server_instance_id": require_nonempty_string(
            server_process.get("server_instance_id"),
            f"{label} server instance",
        ),
        "executable_sha256": verified_process["executable_sha256"],
        "waiter": ExactProcessExitWaiter(pid, process_start_id, target_os),
    }
    waiters.append(entry)
    return entry


def native_server_exit_wait_required(target_os: str, proof_tier: str) -> bool:
    return target_os == "windows" and proof_tier in {
        "calibration",
        "hosted_package",
        "protected_hardware",
        "installed_runtime",
    }


def native_server_exit_wait_budget(manifest: dict) -> dict:
    product_idle_timeout_ms = require_positive_int(
        manifest["server_proof"].get("idle_timeout_ms"),
        "native server product idle timeout",
    )
    return {
        "product_idle_timeout_ms": product_idle_timeout_ms,
        "native_teardown_grace_ms": NATIVE_SERVER_TEARDOWN_GRACE_MS,
        "timeout_ms": product_idle_timeout_ms + NATIVE_SERVER_TEARDOWN_GRACE_MS,
    }


def remaining_native_server_exit_wait_ms(
    deadline: float,
    timeout_ms: int,
    *,
    now: float | None = None,
) -> int:
    current = time.monotonic() if now is None else now
    remaining_ms = math.ceil((deadline - current) * 1000)
    require(
        remaining_ms > 0,
        f"native server cleanup exceeded shared {timeout_ms}ms exit-wait bound",
    )
    return min(timeout_ms, remaining_ms)


def retained_final_native_server_exit_evidence(
    evidence: dict,
    final_entry: dict,
    wait_budget: dict,
    *,
    authenticated_process_count: int,
    superseded_process_count: int,
) -> dict:
    require(
        evidence.get("status") == "normal_idle_exit"
        and evidence.get("exit_code") == 0
        and evidence.get("clean_exit_required") is True,
        "final authenticated server did not prove its clean idle exit",
    )
    require(
        (evidence.get("pid"), evidence.get("process_start_id"))
        == final_entry.get("identity"),
        "final server cleanup evidence changed exact process identity",
    )
    executable_sha256 = require_sha256(
        final_entry.get("executable_sha256"),
        "final server cleanup executable sha256",
    )
    process_wait_timeout_ms = require_positive_int(
        evidence.get("timeout_ms"),
        "final server cleanup process wait timeout",
    )
    require(
        process_wait_timeout_ms <= wait_budget.get("timeout_ms"),
        "final server cleanup process wait exceeded its shared bound",
    )
    return {
        **evidence,
        "observation": "final_temporary_directory_boundary",
        "server_instance_id": final_entry["server_instance_id"],
        "executable_sha256": executable_sha256,
        "process_wait_timeout_ms": process_wait_timeout_ms,
        **wait_budget,
        "authenticated_process_count": authenticated_process_count,
        "superseded_process_count": superseded_process_count,
    }


def _cleanup_environment(env: dict[str, str], control: dict) -> dict[str, str]:
    cleanup_env = dict(env)
    cleanup_env.update(
        {
            "CODESTORY_EMBED_QUALIFICATION_DIR": require_nonempty_string(
                control["qualification_directory"],
                "final server cleanup qualification directory",
            ),
            "CODESTORY_EMBED_QUALIFICATION_NONCE": require_nonempty_string(
                control["qualification_nonce"],
                "final server cleanup qualification nonce",
            ),
        }
    )
    cleanup_env.pop("CODESTORY_CLI", None)
    archive_sha256 = control.get("plugin_cli_archive_sha256")
    if archive_sha256 is not None:
        cleanup_env["CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"] = require_sha256(
            archive_sha256,
            "final server cleanup archive sha256",
        )
    else:
        cleanup_env.pop("CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256", None)
    return cleanup_env


def _observe_final_server(
    args: argparse.Namespace,
    env: dict[str, str],
    control: dict,
    manifest: dict,
    target_os: str,
    *,
    require_final_server: bool,
) -> tuple[dict | None, BaseException | None, BaseException | None]:
    host = None
    final_entry = None
    observation_error = None
    host_close_error = None
    try:
        qualification_cli = Path(control["qualification_cli"]).resolve()
        projects = control["projects"]
        require(
            qualification_cli.is_file()
            and isinstance(projects, list)
            and len(projects) == 2,
            "runtime proof supplied invalid final server cleanup context",
        )
        project = Path(projects[0]).resolve()
        host = McpProcess(
            [
                str(qualification_cli),
                "serve",
                "--stdio",
                "--multi-project",
                "--refresh",
                "none",
            ],
            env=_cleanup_environment(env, control),
            cwd=project,
            timeout=args.timeout_secs,
        )
        host.initialize()
        diagnostics = host.engine_diagnostics(project, "final-cleanup-diagnostics")
        raw_snapshot = find_value(diagnostics, "embedding_server")
        require(
            raw_snapshot is not None or not require_final_server,
            "final package server was absent before its clean exit could be authenticated",
        )
        if raw_snapshot is not None:
            require(
                isinstance(raw_snapshot, dict),
                "final cleanup diagnostics returned a malformed server snapshot",
            )
            snapshot = server_snapshot(diagnostics, manifest, require_resident=False)
            final_entry = pin_temporary_package_server(
                control,
                snapshot["process"],
                manifest,
                target_os,
                "final temporary package embedding server",
            )
    except BaseException as error:
        observation_error = error
    finally:
        if host is not None:
            try:
                host.close()
            except BaseException as error:
                host_close_error = error
    return final_entry, observation_error, host_close_error


def _wait_for_pinned_servers(
    entries: list,
    final_entry: dict | None,
    manifest: dict,
    *,
    require_final_server: bool,
) -> tuple[dict, dict, int, list[str]]:
    wait_budget = native_server_exit_wait_budget(manifest)
    timeout_ms = wait_budget["timeout_ms"]
    final_identity = final_entry.get("identity") if final_entry is not None else None
    entries.sort(
        key=lambda entry: (
            0
            if isinstance(entry, dict) and entry.get("identity") == final_identity
            else 1
        )
    )
    shared_deadline = time.monotonic() + (timeout_ms / 1000)
    exit_evidence = {}
    errors = []
    superseded_process_count = 0
    for entry in entries:
        waiter = entry.get("waiter") if isinstance(entry, dict) else None
        if not isinstance(waiter, ExactProcessExitWaiter):
            errors.append("temporary package cleanup waiter was malformed")
            continue
        superseded = final_identity is not None and entry.get("identity") != final_identity
        if superseded:
            superseded_process_count += 1
        try:
            process_timeout_ms = remaining_native_server_exit_wait_ms(
                shared_deadline,
                timeout_ms,
            )
            exit_evidence[entry["identity"]] = waiter.wait(
                process_timeout_ms,
                require_clean_exit=require_final_server and not superseded,
            )
        except BaseException as error:
            errors.append(str(error))
        finally:
            try:
                waiter.close()
            except BaseException as error:
                errors.append(f"could not close exact process waiter: {error}")
    return exit_evidence, wait_budget, superseded_process_count, errors


def wait_for_final_temporary_package_server(
    args: argparse.Namespace,
    env: dict[str, str],
    control: dict,
    manifest: dict,
    *,
    require_final_server: bool,
) -> dict | None:
    target_os = TARGET_CONTRACTS[manifest["asset_target"]]["target_os"]
    if not native_server_exit_wait_required(target_os, args.proof_tier):
        return None
    waiters = control.setdefault("_waiters", [])
    require(isinstance(waiters, list), "temporary package cleanup waiter state is invalid")
    configured = all(
        control.get(field) is not None
        for field in (
            "qualification_cli",
            "qualification_directory",
            "qualification_nonce",
            "projects",
        )
    )
    if configured:
        final_entry, observation_error, host_close_error = _observe_final_server(
            args,
            env,
            control,
            manifest,
            target_os,
            require_final_server=require_final_server,
        )
    else:
        final_entry = None
        host_close_error = None
        observation_error = (
            ProofFailure("runtime proof omitted final server cleanup context")
            if require_final_server
            else None
        )
    entries = list(waiters)
    control["_waiters"] = []
    evidence, wait_budget, superseded_count, waiter_errors = _wait_for_pinned_servers(
        entries,
        final_entry,
        manifest,
        require_final_server=require_final_server,
    )
    failures = []
    if observation_error is not None:
        failures.append(f"final server observation failed: {observation_error}")
    if host_close_error is not None:
        failures.append(f"final observational client close failed: {host_close_error}")
    failures.extend(waiter_errors)
    require(not failures, "; ".join(failures))
    if not require_final_server or final_entry is None:
        return None
    final_evidence = evidence.get(final_entry["identity"])
    require(isinstance(final_evidence, dict), "final server cleanup lost its exit evidence")
    return retained_final_native_server_exit_evidence(
        final_evidence,
        final_entry,
        wait_budget,
        authenticated_process_count=len(entries),
        superseded_process_count=superseded_count,
    )
