"""Best-effort preservation of qualification evidence for failed proof runs.

The packaged proof executes inside a temporary package root that is destroyed
when the run ends. ``FailurePreservingTemporaryDirectory`` preserves the
primary *exception* when cleanup itself fails, but the evidence written under
the temporary root (``qualification-suite/artifacts/request.json``, driver
output, worker logs, partial measurement samples) is still deleted with the
process. This module copies that evidence into the workflow-uploaded output
directory on the failure path, so a post-mortem never requires a re-run.

Preservation is diagnostic and best-effort: it never masks the primary proof
failure, it adds nothing on green runs, and per-run secrets registered by the
producing phases (the qualification nonces) are redacted from every preserved
byte before it can reach an uploaded artifact.
"""

from __future__ import annotations

import json
from pathlib import Path

from .subprocess_control import add_exception_note

EVIDENCE_DIRECTORY_NAMES = ("qualification-suite", "qualification")
FAILURE_EVIDENCE_DIRECTORY_NAME = "failure-evidence"
REDACTION_MARKER = "[redacted-qualification-secret]"
PRESERVATION_BYTE_BUDGET = 64 * 1024 * 1024

_registered_secrets: list[str] = []


def register_failure_evidence_secret(value: str) -> None:
    """Record a per-run secret so preserved evidence never contains it."""
    if isinstance(value, str) and value and value not in _registered_secrets:
        _registered_secrets.append(value)


def reset_failure_evidence_secrets() -> None:
    """Clear registered secrets; self-test isolation only."""
    _registered_secrets.clear()


def _redacted(payload: bytes) -> bytes:
    marker = REDACTION_MARKER.encode("utf-8")
    for secret in _registered_secrets:
        payload = payload.replace(secret.encode("utf-8"), marker)
    return payload


def _preserve_directory(
    source: Path,
    destination: Path,
    remaining_budget: int,
    skipped: list[str],
) -> int:
    for path in sorted(source.rglob("*")):
        relative = path.relative_to(source).as_posix()
        try:
            if path.is_symlink():
                skipped.append(f"{relative} (symlink)")
                continue
            if path.is_dir():
                (destination / relative).mkdir(parents=True, exist_ok=True)
                continue
            if not path.is_file():
                skipped.append(f"{relative} (special file)")
                continue
            size = path.stat().st_size
            if size > remaining_budget:
                skipped.append(f"{relative} (over evidence budget: {size} bytes)")
                continue
            payload = _redacted(path.read_bytes())
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(payload)
            remaining_budget -= size
        except OSError as copy_error:
            skipped.append(f"{relative} (unreadable: {copy_error})")
    return remaining_budget


def preserve_failure_evidence(
    root: Path,
    out_dir: Path,
    error: BaseException,
) -> None:
    """Copy qualification evidence from ``root`` into ``out_dir`` on failure.

    ``root`` is the temporary package root that is about to be destroyed;
    ``out_dir`` is the ``--out-dir`` upload root that consuming workflows
    already upload. Nothing is written when no qualification evidence exists
    yet, so failures before the qualification suite stays artifact-free and
    green runs are untouched. Preservation never raises past this function.
    """
    try:
        sources = [
            root / name
            for name in EVIDENCE_DIRECTORY_NAMES
            if (root / name).is_dir() and not (root / name).is_symlink()
        ]
        if not sources:
            return
        destination = out_dir / FAILURE_EVIDENCE_DIRECTORY_NAME
        skipped: list[str] = []
        remaining_budget = PRESERVATION_BYTE_BUDGET
        for source in sources:
            remaining_budget = _preserve_directory(
                source,
                destination / source.name,
                remaining_budget,
                skipped,
            )
        manifest = {
            "schema_version": 1,
            "reason": (
                "packaged proof failed after qualification evidence existed; "
                "the temporary package root is destroyed, so its evidence is "
                "copied here for post-mortems"
            ),
            "error": _redacted(
                f"{type(error).__name__}: {error}".encode("utf-8", "replace")
            ).decode("utf-8", "replace"),
            "preserved_directories": [source.name for source in sources],
            "skipped_entries": skipped,
            "redaction_marker": REDACTION_MARKER,
            "redacted_secret_count": len(_registered_secrets),
        }
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "failure.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        add_exception_note(
            error,
            f"qualification failure evidence preserved under {destination}",
        )
    except BaseException as preservation_error:  # never mask the proof failure
        try:
            add_exception_note(
                error,
                f"failure evidence preservation also failed: {preservation_error}",
            )
        except BaseException:
            pass
