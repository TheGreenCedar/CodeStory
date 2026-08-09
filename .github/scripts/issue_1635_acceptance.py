#!/usr/bin/env python3
"""Validate the exact Windows linker-timing artifact for issue #1635.

The producer deliberately treats missing linker timing as observational. This
consumer has a narrower purpose: it decides whether the frozen candidate has
the exact positive evidence needed to close #1635. It therefore emits a reject
receipt for unavailable or malformed timing without making any claim about the
package's validity.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import stat
import sys
import tempfile
import zipfile
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any, BinaryIO


RECEIPT_SCHEMA = "codestory.issue-1635-acceptance/v1"
TIMING_SCHEMA = "codestory.windows-link-timing/v1"
BUILD_IDENTITY_SCHEMA = "codestory.cargo-build-artifacts/v2"
WORKFLOW_PATH = ".github/workflows/packaged-platform-pr.yml"
LINK_PHASE = "msvc_link"
BUILD_PHASE = "cargo_graph"
WINDOWS_TARGET = "windows-x64"
WINDOWS_RUST_TARGET = "x86_64-pc-windows-msvc"
QUALIFICATION_ARTIFACT = "codestory-qualification-driver-windows-x64"
WINDOWS_JOB = "packaged-proof / Build windows-x64"
WINDOWS_JOB_STEPS = (
    "Require exact source identity",
    "Build package and qualification driver",
    "Stage qualification driver in package proof artifact",
    "Upload separate qualification driver",
    "Upload Windows package build timing",
)
QUALIFICATION_IDENTITY_MEMBER = "qualification-driver-identity.json"
QUALIFICATION_DRIVER_MEMBER = "codestory_embedding_qualification.exe"
BUILD_IDENTITY_MEMBER = "cargo-build-artifacts.json"
TIMING_MEMBER = "windows-link-timing.json"
LINK_LOG_MEMBER = "msvc-link-time.log"

SHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
ACTION_DIGEST = re.compile(r"^sha256:([0-9a-f]{64})$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_LINK_LOG_BYTES = 128 * 1024 * 1024
SECOND_TOLERANCE = 1e-6
PASS_INTERVAL = re.compile(
    r"(?:^|[^0-9A-Za-z_])Pass\s+([0-9]{1,3}):\s+Interval\s+#([0-9]{1,3}),"
    r"\s+time\s*=\s*([0-9]+(?:\.[0-9]+)?)s(?![0-9A-Za-z_.])"
)
FINAL_TOTAL = re.compile(
    r"(?:^|[^0-9A-Za-z_])Final:\s+Total\s+time\s*=\s*"
    r"([0-9]+(?:\.[0-9]+)?)s(?![0-9A-Za-z_.])"
)


class EvidenceError(ValueError):
    """A fail-closed evidence rejection with a receipt-safe message."""


def _fail(message: str) -> None:
    raise EvidenceError(message)


def _object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        _fail(f"{label} must be an object")
    return value


def _exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    selected = _object(value, label)
    if set(selected) != expected:
        _fail(f"{label} keys changed")
    return selected


def _text(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value or value.strip() != value:
        _fail(f"{label} must be non-empty trimmed text")
    return value


def _positive_integer(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        _fail(f"{label} must be a positive integer")
    return value


def _positive_bytes(value: Any, label: str) -> int:
    selected = _positive_integer(value, label)
    if selected > 2**53 - 1:
        _fail(f"{label} exceeds the portable safe-integer range")
    return selected


def _sha(value: Any, label: str) -> str:
    selected = _text(value, label)
    if not SHA.fullmatch(selected):
        _fail(f"{label} must be a full lowercase Git digest")
    return selected


def _sha256(value: Any, label: str) -> str:
    selected = _text(value, label)
    if not SHA256.fullmatch(selected):
        _fail(f"{label} must be a lowercase SHA-256 digest")
    return selected


def _finite_nonnegative(value: Any, label: str) -> int | float:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or value < 0
    ):
        _fail(f"{label} must be a finite non-negative number")
    return value


def _timestamp(value: Any, label: str) -> str:
    selected = _text(value, label)
    try:
        datetime.fromisoformat(selected.replace("Z", "+00:00"))
    except ValueError:
        _fail(f"{label} must be an ISO timestamp")
    return selected


def _timestamp_value(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def _regular_file(path_value: str | os.PathLike[str], label: str) -> Path:
    path = Path(path_value).absolute()
    try:
        metadata = path.lstat()
    except OSError as error:
        _fail(f"{label} is missing or unreadable: {error.strerror or error}")
    if path.is_symlink() or not stat.S_ISREG(metadata.st_mode):
        _fail(f"{label} must be a regular non-symlink file")
    return path


def _parse_json(value: str, label: str) -> dict[str, Any]:
    def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        selected: dict[str, Any] = {}
        for key, item in pairs:
            if key in selected:
                _fail(f"{label} contains duplicate key {key}")
            selected[key] = item
        return selected

    def invalid_constant(constant: str) -> None:
        _fail(f"{label} contains non-finite JSON number {constant}")

    try:
        parsed = json.loads(
            value,
            object_pairs_hook=unique_object,
            parse_constant=invalid_constant,
        )
    except json.JSONDecodeError as error:
        _fail(f"{label} is not valid JSON: {error}")
    return _object(parsed, label)


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _load_json_file(path_value: str | os.PathLike[str], label: str) -> dict[str, Any]:
    path = _regular_file(path_value, label)
    size = path.stat().st_size
    if size <= 0 or size > MAX_JSON_BYTES:
        _fail(f"{label} must be a non-empty bounded JSON file")
    try:
        return _parse_json(path.read_text(encoding="utf-8"), label)
    except (OSError, UnicodeError) as error:
        _fail(f"{label} is not valid UTF-8 JSON: {error}")


def _safe_zip_members(container: zipfile.ZipFile, label: str) -> dict[str, zipfile.ZipInfo]:
    members: dict[str, zipfile.ZipInfo] = {}
    folded: set[str] = set()
    for info in container.infolist():
        name = info.filename
        path = PurePosixPath(name)
        mode = info.external_attr >> 16
        if (
            not name
            or "\\" in name
            or "\x00" in name
            or path.is_absolute()
            or any(part in {"", ".", ".."} for part in path.parts)
            or (stat.S_IFMT(mode) not in {0, stat.S_IFREG, stat.S_IFDIR})
            or info.flag_bits & 0x1
        ):
            _fail(f"{label} contains an unsafe member")
        folded_name = name.casefold()
        if name in members or folded_name in folded:
            _fail(f"{label} contains duplicate Windows member {name}")
        members[name] = info
        folded.add(folded_name)
    return members


def _read_zip_member(
    container: zipfile.ZipFile,
    members: dict[str, zipfile.ZipInfo],
    name: str,
    label: str,
    max_bytes: int = MAX_JSON_BYTES,
) -> bytes:
    info = members.get(name)
    if info is None or info.is_dir():
        _fail(f"{label} is missing")
    if info.file_size <= 0 or info.file_size > max_bytes:
        _fail(f"{label} must be non-empty and bounded")
    try:
        value = container.read(info)
    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
        _fail(f"{label} is unreadable: {error}")
    if len(value) != info.file_size:
        _fail(f"{label} changed while it was read")
    return value


def _text_zip_member(
    container: zipfile.ZipFile,
    members: dict[str, zipfile.ZipInfo],
    name: str,
    label: str,
) -> tuple[str, dict[str, Any]]:
    value = _read_zip_member(
        container,
        members,
        name,
        label,
        max_bytes=MAX_LINK_LOG_BYTES,
    )
    try:
        text = value.decode("utf-8")
    except UnicodeError as error:
        _fail(f"{label} is not valid UTF-8: {error}")
    return text, {
        "member": name,
        "bytes": len(value),
        "sha256": hashlib.sha256(value).hexdigest(),
    }


def _json_zip_member(
    container: zipfile.ZipFile,
    members: dict[str, zipfile.ZipInfo],
    name: str,
    label: str,
) -> tuple[dict[str, Any], dict[str, Any]]:
    value = _read_zip_member(container, members, name, label)
    try:
        parsed = _parse_json(value.decode("utf-8"), label)
    except UnicodeError as error:
        _fail(f"{label} is not valid UTF-8 JSON: {error}")
    return parsed, {
        "member": name,
        "bytes": len(value),
        "sha256": hashlib.sha256(value).hexdigest(),
    }


def _hash_zip_member(
    container: zipfile.ZipFile,
    members: dict[str, zipfile.ZipInfo],
    name: str,
    label: str,
) -> dict[str, Any]:
    info = members.get(name)
    if info is None or info.is_dir() or info.file_size <= 0:
        _fail(f"{label} is missing or empty")
    digest = hashlib.sha256()
    try:
        with container.open(info, "r") as source:
            _hash_stream(source, digest)
    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
        _fail(f"{label} is unreadable: {error}")
    return {"member": name, "bytes": info.file_size, "sha256": digest.hexdigest()}


def _hash_stream(source: BinaryIO, digest: Any) -> None:
    for chunk in iter(lambda: source.read(1024 * 1024), b""):
        digest.update(chunk)


def _expected_identity(
    *,
    repository: str,
    commit: str,
    tree: str,
    version: str,
    run_id: int,
    run_attempt: int,
) -> dict[str, Any]:
    if not REPOSITORY.fullmatch(_text(repository, "repository")):
        _fail("repository must be owner/name")
    normalized_version = version.removeprefix("v")
    if not VERSION.fullmatch(normalized_version):
        _fail("version must be plain semantic version text")
    return {
        "repository": repository,
        "source": {
            "commit": _sha(commit, "expected commit"),
            "tree": _sha(tree, "expected tree"),
        },
        "release_version": normalized_version,
        "producer": {
            "workflow": WORKFLOW_PATH,
            "run_id": _positive_integer(run_id, "expected run id"),
            "run_attempt": _positive_integer(run_attempt, "expected run attempt"),
            "job_name": WINDOWS_JOB,
        },
    }


def _validate_run(run: dict[str, Any], expected: dict[str, Any]) -> dict[str, Any]:
    producer = expected["producer"]
    if (
        run.get("id") != producer["run_id"]
        or run.get("run_attempt") != producer["run_attempt"]
        or run.get("head_sha") != expected["source"]["commit"]
        or _object(run.get("head_commit"), "Actions run head_commit").get("tree_id")
        != expected["source"]["tree"]
        or _object(run.get("head_repository"), "Actions run head_repository").get("full_name")
        != expected["repository"]
        or _object(run.get("repository"), "Actions run repository").get("full_name")
        != expected["repository"]
        or run.get("path") != WORKFLOW_PATH
        or run.get("event") != "workflow_dispatch"
    ):
        _fail("Actions run identity does not match the exact qualification run")
    if run.get("status") not in {"in_progress", "completed"}:
        _fail("Actions qualification run is not active or completed")
    if run.get("conclusion") not in {None, "success"}:
        _fail("Actions qualification run has a non-success conclusion")
    expected_url = (
        f"https://github.com/{expected['repository']}/actions/runs/{producer['run_id']}"
    )
    if run.get("html_url") != expected_url:
        _fail("Actions qualification run URL changed")
    return {
        "workflow": WORKFLOW_PATH,
        "run_id": producer["run_id"],
        "run_attempt": producer["run_attempt"],
        "run_url": expected_url,
    }


def _select_job(
    jobs_document: dict[str, Any], expected: dict[str, Any]
) -> tuple[dict[str, Any], datetime, datetime]:
    jobs = jobs_document.get("jobs")
    if not isinstance(jobs, list):
        _fail("Actions job evidence must contain a jobs array")
    if jobs_document.get("total_count") != len(jobs):
        _fail("Actions job evidence is truncated")
    producer = expected["producer"]
    matching = [
        row
        for row in jobs
        if isinstance(row, dict)
        and row.get("name") == producer["job_name"]
        and row.get("run_attempt") == producer["run_attempt"]
    ]
    if len(matching) != 1:
        _fail("Actions evidence must contain exactly one expected Windows build job")
    job = matching[0]
    if (
        job.get("run_id") != producer["run_id"]
        or job.get("run_attempt") != producer["run_attempt"]
        or job.get("head_sha") != expected["source"]["commit"]
        or job.get("status") != "completed"
        or job.get("conclusion") != "success"
    ):
        _fail("Actions Windows build job is not a successful exact-run execution")
    steps = job.get("steps")
    if not isinstance(steps, list):
        _fail("Actions Windows build job has no step evidence")
    for required_step in WINDOWS_JOB_STEPS:
        matching_steps = [
            step
            for step in steps
            if isinstance(step, dict) and step.get("name") == required_step
        ]
        if len(matching_steps) != 1 or matching_steps[0].get("conclusion") != "success":
            _fail(f"Actions Windows build job did not succeed at {required_step}")
    started_at = _timestamp(job.get("started_at"), "Actions job started_at")
    completed_at = _timestamp(job.get("completed_at"), "Actions job completed_at")
    started = _timestamp_value(started_at)
    completed = _timestamp_value(completed_at)
    if completed < started:
        _fail("Actions job completion precedes its start")
    return {
        "job_id": _positive_integer(job.get("id"), "Actions job id"),
        "job_name": producer["job_name"],
        "started_at": started_at,
        "completed_at": completed_at,
    }, started, completed


def _select_container(
    artifacts_document: dict[str, Any],
    *,
    expected: dict[str, Any],
    job_started: datetime,
    job_completed: datetime,
    name: str,
    label: str,
) -> dict[str, Any]:
    artifacts = artifacts_document.get("artifacts")
    if not isinstance(artifacts, list):
        _fail("Actions artifact evidence must contain an artifacts array")
    if artifacts_document.get("total_count") != len(artifacts):
        _fail("Actions artifact evidence is truncated")
    matching = [row for row in artifacts if isinstance(row, dict) and row.get("name") == name]
    if len(matching) != 1:
        _fail(f"Actions run must retain exactly one {name} artifact")
    artifact = matching[0]
    workflow_run = _object(artifact.get("workflow_run"), f"{label} workflow_run")
    producer = expected["producer"]
    if (
        artifact.get("expired") is not False
        or workflow_run.get("id") != producer["run_id"]
        or workflow_run.get("head_sha") != expected["source"]["commit"]
    ):
        _fail(f"{label} artifact has stale run or head identity")
    digest_match = ACTION_DIGEST.fullmatch(str(artifact.get("digest", "")))
    if digest_match is None:
        _fail(f"{label} artifact has no exact container digest")
    created_at = _timestamp(artifact.get("created_at"), f"{label} artifact created_at")
    created = _timestamp_value(created_at)
    if created < job_started or created > job_completed:
        _fail(f"{label} artifact was not created by the selected job window")
    return {
        "id": _positive_integer(artifact.get("id"), f"{label} artifact id"),
        "name": name,
        "bytes": _positive_bytes(artifact.get("size_in_bytes"), f"{label} artifact bytes"),
        "sha256": digest_match.group(1),
        "created_at": created_at,
    }


def _validate_container_file(path_value: str, identity: dict[str, Any], label: str) -> Path:
    path = _regular_file(path_value, f"{label} container")
    if path.stat().st_size != identity["bytes"] or _file_sha256(path) != identity["sha256"]:
        _fail(f"{label} container bytes do not match the authenticated Actions artifact")
    if not zipfile.is_zipfile(path):
        _fail(f"{label} container is not a ZIP artifact")
    return path


def _validate_build_identity(
    build: dict[str, Any], expected: dict[str, Any]
) -> dict[str, Any]:
    build = _exact_keys(build, {"schema", "source", "build", "artifacts"}, "build identity")
    if build.get("schema") != BUILD_IDENTITY_SCHEMA:
        _fail("build identity schema changed")
    source = _exact_keys(build.get("source"), {"commit", "tree"}, "build identity source")
    if source != expected["source"]:
        _fail("build identity source does not match the exact head and tree")
    build_contract = _exact_keys(
        build.get("build"),
        {"rust_target", "profile", "target_dir", "workspace_root"},
        "build identity build contract",
    )
    if (
        build_contract.get("rust_target") != WINDOWS_RUST_TARGET
        or build_contract.get("profile") != "release"
    ):
        _fail("build identity is not the Windows release build")
    artifacts = _object(build.get("artifacts"), "build identity artifacts")
    if set(artifacts) != {"cli", "runtime", "qualification_driver"}:
        _fail("build identity does not contain the exact qualification release graph")
    driver = _object(artifacts.get("qualification_driver"), "build qualification driver")
    if (
        driver.get("package") != "codestory-bench"
        or driver.get("target") != "codestory_embedding_qualification"
        or driver.get("kind") != "bin"
        or driver.get("relative_path") != QUALIFICATION_DRIVER_MEMBER
        or _object(driver.get("profile"), "build qualification profile").get("test") is not False
    ):
        _fail("build qualification driver contract changed")
    return {
        "file": QUALIFICATION_DRIVER_MEMBER,
        "bytes": _positive_bytes(driver.get("bytes"), "build qualification driver bytes"),
        "sha256": _sha256(driver.get("sha256"), "build qualification driver sha256"),
    }


def _validate_qualification_identity(
    identity: dict[str, Any], expected: dict[str, Any]
) -> tuple[dict[str, Any], dict[str, Any]]:
    identity = _exact_keys(
        identity,
        {"schema_version", "source", "release_version", "asset_target", "archive", "driver"},
        "qualification identity",
    )
    source = _exact_keys(identity.get("source"), {"commit", "tree"}, "qualification source")
    archive = _exact_keys(identity.get("archive"), {"file", "bytes", "sha256"}, "archive identity")
    driver = _exact_keys(identity.get("driver"), {"file", "bytes", "sha256"}, "qualification driver identity")
    expected_archive = f"codestory-cli-v{expected['release_version']}-{WINDOWS_TARGET}.zip"
    if (
        identity.get("schema_version") != 1
        or source != expected["source"]
        or identity.get("release_version") != expected["release_version"]
        or identity.get("asset_target") != WINDOWS_TARGET
        or archive.get("file") != expected_archive
        or driver.get("file") != QUALIFICATION_DRIVER_MEMBER
    ):
        _fail("qualification identity does not match the exact Windows candidate")
    return (
        {
            "name": expected_archive,
            "bytes": _positive_bytes(archive.get("bytes"), "candidate archive bytes"),
            "sha256": _sha256(archive.get("sha256"), "candidate archive sha256"),
        },
        {
            "file": QUALIFICATION_DRIVER_MEMBER,
            "bytes": _positive_bytes(driver.get("bytes"), "qualification driver bytes"),
            "sha256": _sha256(driver.get("sha256"), "qualification driver sha256"),
        },
    )


def _milliseconds(seconds: float) -> int:
    return math.floor(seconds * 1000 + 0.5)


def _parse_link_log(value: str) -> dict[str, Any]:
    invocations: list[dict[str, Any]] = []
    open_intervals: list[dict[str, Any]] | None = None
    incoherent = 0
    truncated = 0
    orphan_totals = 0
    dangling_intervals = 0

    for line in value.splitlines():
        interval_match = PASS_INTERVAL.search(line)
        if interval_match is not None:
            pass_number = int(interval_match.group(1))
            interval_number = int(interval_match.group(2))
            seconds = float(interval_match.group(3))
            if not math.isfinite(seconds) or seconds < 0:
                _fail("retained linker log contains an invalid interval duration")
            interval = {
                "pass": pass_number,
                "interval": interval_number,
                "seconds": seconds,
            }
            if pass_number == 1 and interval_number == 1:
                if open_intervals is not None:
                    truncated += 1
                open_intervals = [interval]
                continue
            if open_intervals is None:
                dangling_intervals += 1
                continue
            previous = open_intervals[-1]
            if (
                pass_number <= previous["pass"]
                or interval_number <= previous["interval"]
            ):
                incoherent += 1
                open_intervals = None
                continue
            open_intervals.append(interval)
            continue

        total_match = FINAL_TOTAL.search(line)
        if total_match is None:
            continue
        total_seconds = float(total_match.group(1))
        if not math.isfinite(total_seconds) or total_seconds < 0:
            _fail("retained linker log contains an invalid total duration")
        if open_intervals is None:
            orphan_totals += 1
            continue
        measured = sum(interval["seconds"] for interval in open_intervals)
        if total_seconds + SECOND_TOLERANCE < measured or total_seconds <= 0:
            incoherent += 1
            open_intervals = None
            continue
        invocations.append({
            "index": len(invocations) + 1,
            "intervals": open_intervals,
            "total_seconds": total_seconds,
            "total_ms": _milliseconds(total_seconds),
        })
        open_intervals = None

    if open_intervals is not None:
        truncated += 1

    diagnostics = {
        "truncated_reports": truncated,
        "orphan_totals": orphan_totals,
        "dangling_intervals": dangling_intervals,
    }
    if incoherent > 0:
        _fail("retained linker log contains an incoherent explicit linker report")
    if not invocations:
        _fail("retained linker log contains no complete explicit linker invocation")
    return {
        "invocations": invocations,
        "invocation_count": len(invocations),
        "link_ms": _milliseconds(
            sum(invocation["total_seconds"] for invocation in invocations)
        ),
        "diagnostics": diagnostics,
    }


def _validate_timing(
    timing: dict[str, Any], retained_link_evidence: dict[str, Any]
) -> dict[str, Any]:
    timing = _exact_keys(
        timing,
        {
            "schema",
            "phase",
            "source",
            "status",
            "reason",
            "observational",
            "invocation_count",
            "link_ms",
            "invocations",
            "build_interval",
            "diagnostics",
        },
        "Windows link timing",
    )
    if timing.get("schema") != TIMING_SCHEMA:
        _fail("Windows link timing schema is not codestory.windows-link-timing/v1")
    if timing.get("phase") != LINK_PHASE:
        _fail("Windows link timing phase is not msvc_link")
    if timing.get("status") != "observed":
        _fail("Windows link timing status is not observed")
    if timing.get("source") != "msvc-link-time-report" or timing.get("observational") is not True:
        _fail("Windows link timing evidence boundary changed")
    if timing.get("reason") is not None:
        _fail("observed Windows link timing must not carry an unavailable reason")
    invocation_count = _positive_integer(
        timing.get("invocation_count"), "Windows link timing invocation_count"
    )
    invocations = timing.get("invocations")
    if not isinstance(invocations, list) or len(invocations) != invocation_count:
        _fail("Windows link timing invocations do not match invocation_count")
    link_ms = _finite_nonnegative(timing.get("link_ms"), "Windows link timing link_ms")
    build_interval = _exact_keys(
        timing.get("build_interval"), {"phase", "elapsed_ms"}, "Windows build interval"
    )
    build_ms = _finite_nonnegative(build_interval.get("elapsed_ms"), "Windows build elapsed_ms")
    if build_interval.get("phase") != BUILD_PHASE or link_ms > build_ms:
        _fail("Windows link timing is not bounded by the cargo_graph interval")
    _object(timing.get("diagnostics"), "Windows link timing diagnostics")
    if (
        invocation_count != retained_link_evidence["invocation_count"]
        or link_ms != retained_link_evidence["link_ms"]
        or invocations != retained_link_evidence["invocations"]
        or timing.get("diagnostics") != retained_link_evidence["diagnostics"]
    ):
        _fail("Windows link timing JSON does not match the retained linker log")
    return {
        "schema": TIMING_SCHEMA,
        "phase": LINK_PHASE,
        "status": "observed",
        "invocation_count": invocation_count,
        "link_ms": link_ms,
        "build_elapsed_ms": build_ms,
    }


def _validate_evidence(
    *,
    expected: dict[str, Any],
    run_evidence: str,
    job_evidence: str,
    artifact_evidence: str,
    qualification_container: str,
    timing_container: str,
) -> dict[str, Any]:
    run = _load_json_file(run_evidence, "Actions run evidence")
    jobs = _load_json_file(job_evidence, "Actions job evidence")
    artifacts = _load_json_file(artifact_evidence, "Actions artifact evidence")
    producer = _validate_run(run, expected)
    job, job_started, job_completed = _select_job(jobs, expected)
    producer.update(job)

    timing_name = f"windows-package-build-timing-attempt-{expected['producer']['run_attempt']}"
    qualification_container_identity = _select_container(
        artifacts,
        expected=expected,
        job_started=job_started,
        job_completed=job_completed,
        name=QUALIFICATION_ARTIFACT,
        label="qualification",
    )
    timing_container_identity = _select_container(
        artifacts,
        expected=expected,
        job_started=job_started,
        job_completed=job_completed,
        name=timing_name,
        label="timing",
    )
    if qualification_container_identity["id"] == timing_container_identity["id"]:
        _fail("qualification and timing evidence must use distinct artifact containers")

    qualification_path = _validate_container_file(
        qualification_container, qualification_container_identity, "qualification"
    )
    timing_path = _validate_container_file(timing_container, timing_container_identity, "timing")

    try:
        with zipfile.ZipFile(qualification_path, "r") as qualification_zip:
            qualification_members = _safe_zip_members(qualification_zip, "qualification container")
            if set(qualification_members) != {
                QUALIFICATION_IDENTITY_MEMBER,
                QUALIFICATION_DRIVER_MEMBER,
            }:
                _fail("qualification container members changed")
            qualification_identity, qualification_identity_file = _json_zip_member(
                qualification_zip,
                qualification_members,
                QUALIFICATION_IDENTITY_MEMBER,
                "qualification identity",
            )
            qualification_driver_file = _hash_zip_member(
                qualification_zip,
                qualification_members,
                QUALIFICATION_DRIVER_MEMBER,
                "qualification driver",
            )

        with zipfile.ZipFile(timing_path, "r") as timing_zip:
            timing_members = _safe_zip_members(timing_zip, "timing container")
            build_identity, build_identity_file = _json_zip_member(
                timing_zip, timing_members, BUILD_IDENTITY_MEMBER, "build identity"
            )
            timing, timing_file = _json_zip_member(
                timing_zip, timing_members, TIMING_MEMBER, "Windows link timing"
            )
            link_log, link_log_file = _text_zip_member(
                timing_zip, timing_members, LINK_LOG_MEMBER, "retained linker log"
            )
    except zipfile.BadZipFile as error:
        _fail(f"artifact container is not a readable ZIP: {error}")

    build_driver = _validate_build_identity(build_identity, expected)
    archive, qualification_driver = _validate_qualification_identity(
        qualification_identity, expected
    )
    actual_driver = {
        "file": qualification_driver_file["member"],
        "bytes": qualification_driver_file["bytes"],
        "sha256": qualification_driver_file["sha256"],
    }
    if build_driver != qualification_driver or qualification_driver != actual_driver:
        _fail("qualification artifact does not match the exact Windows build identity")
    retained_link_evidence = _parse_link_log(link_log)
    timing_summary = _validate_timing(timing, retained_link_evidence)

    return {
        "producer": producer,
        "containers": {
            "qualification": qualification_container_identity,
            "timing": timing_container_identity,
        },
        "archive": archive,
        "evidence": {
            "build_identity": build_identity_file,
            "qualification_identity": qualification_identity_file,
            "qualification_driver": qualification_driver_file,
            "timing": {
                "record": timing_file,
                "retained_log": link_log_file,
                **timing_summary,
            },
        },
    }


def assess_issue_1635(
    *,
    repository: str,
    commit: str,
    tree: str,
    version: str,
    run_id: int,
    run_attempt: int,
    run_evidence: str,
    job_evidence: str,
    artifact_evidence: str,
    qualification_container: str,
    timing_container: str,
) -> dict[str, Any]:
    """Return a deterministic accept or reject receipt for one exact candidate."""

    expected = _expected_identity(
        repository=repository,
        commit=commit,
        tree=tree,
        version=version,
        run_id=run_id,
        run_attempt=run_attempt,
    )
    receipt: dict[str, Any] = {
        "schema": RECEIPT_SCHEMA,
        "issue": 1635,
        "decision": "reject",
        "reason": None,
        **expected,
        "containers": None,
        "archive": None,
        "evidence": None,
    }
    try:
        accepted = _validate_evidence(
            expected=expected,
            run_evidence=run_evidence,
            job_evidence=job_evidence,
            artifact_evidence=artifact_evidence,
            qualification_container=qualification_container,
            timing_container=timing_container,
        )
    except EvidenceError as error:
        receipt["reason"] = str(error)
        return receipt

    receipt.update(accepted)
    receipt["decision"] = "accept"
    return receipt


def _write_receipt(path_value: str, receipt: dict[str, Any]) -> None:
    path = Path(path_value).resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and path.is_symlink():
        _fail("receipt output must not be a symbolic link")
    payload = (json.dumps(receipt, indent=2, sort_keys=True, allow_nan=False) + "\n").encode()
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    verify = subparsers.add_parser("verify", help="emit the exact-candidate acceptance receipt")
    verify.add_argument("--repository", required=True)
    verify.add_argument("--commit", required=True)
    verify.add_argument("--tree", required=True)
    verify.add_argument("--version", required=True)
    verify.add_argument("--run-id", required=True, type=int)
    verify.add_argument("--run-attempt", required=True, type=int)
    verify.add_argument("--run-evidence", required=True)
    verify.add_argument("--job-evidence", required=True)
    verify.add_argument("--artifact-evidence", required=True)
    verify.add_argument("--qualification-container", required=True)
    verify.add_argument("--timing-container", required=True)
    verify.add_argument("--out", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        receipt = assess_issue_1635(
            repository=args.repository,
            commit=args.commit,
            tree=args.tree,
            version=args.version,
            run_id=args.run_id,
            run_attempt=args.run_attempt,
            run_evidence=args.run_evidence,
            job_evidence=args.job_evidence,
            artifact_evidence=args.artifact_evidence,
            qualification_container=args.qualification_container,
            timing_container=args.timing_container,
        )
        _write_receipt(args.out, receipt)
    except EvidenceError as error:
        print(str(error), file=sys.stderr)
        return 2
    print(json.dumps(receipt, sort_keys=True, allow_nan=False))
    return 0 if receipt["decision"] == "accept" else 1


if __name__ == "__main__":
    raise SystemExit(main())
