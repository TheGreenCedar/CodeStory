"""One publication fault injection and recovery run."""

from __future__ import annotations

import hashlib
import secrets
import subprocess
from pathlib import Path

from .contracts import write_private_json
from .foundation import require
from .publication_fault_setup import _restore_fixture
from .publication_fault_types import (
    PublicationCandidate,
    PublicationCommands,
    PublicationFaultRun,
    PublicationFixture,
)
from .publication_protocol import (
    read_jsonl,
    run_publication_replacement_worker,
    send_server_qualification_control,
    wait_for_jsonl_event,
)


def _start_fault_candidate(
    env: dict[str, str],
    private_root: Path,
    nonce: str,
    fixture: PublicationFixture,
    commands: PublicationCommands,
) -> PublicationCandidate:
    correlation_id = secrets.token_hex(16)
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    pause_path = private_root / f"publication-pause-{nonce_sha256}.json"
    resume_path = private_root / f"publication-resume-{correlation_id}.json"
    event_path = private_root / f"publication-events-{correlation_id}.jsonl"
    write_private_json(
        pause_path,
        {
            "schema_version": 1,
            "nonce_sha256": nonce_sha256,
            "correlation_id": correlation_id,
            "action": "pause_before_manifest_commit",
        },
    )
    fixture.source_file.write_text(
        fixture.baseline_source
        + "// publication qualification candidate source change\n",
        encoding="utf-8",
    )
    fixture.lexical_file.write_text(
        "# Publication qualification candidate\n",
        encoding="utf-8",
    )
    process = subprocess.Popen(
        commands.retrieval_index,
        cwd=fixture.project,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return PublicationCandidate(
        correlation_id,
        nonce_sha256,
        pause_path,
        resume_path,
        event_path,
        process,
    )


def _run_fault(
    cli: Path,
    env: dict[str, str],
    private_root: Path,
    nonce: str,
    fixture: PublicationFixture,
    commands: PublicationCommands,
    *,
    timeout: int,
) -> PublicationFaultRun:
    snapshot_before = send_server_qualification_control(
        private_root,
        nonce,
        sequence=1,
        action="snapshot",
        timeout=timeout,
    )
    candidate = _start_fault_candidate(
        env,
        private_root,
        nonce,
        fixture,
        commands,
    )
    stdout = ""
    stderr = ""
    try:
        wait_for_jsonl_event(
            candidate.event_path,
            lambda event: (
                event.get("action") == "pause_before_manifest_commit"
                and event.get("status") == "waiting_for_resume"
            ),
            timeout=timeout,
            process=candidate.process,
        )
        send_server_qualification_control(
            private_root,
            nonce,
            sequence=2,
            action="crash_server",
            timeout=timeout,
        )
        run_publication_replacement_worker(
            cli,
            env,
            fixture.project,
            private_root,
            nonce,
            timeout=timeout,
        )
        snapshot_after = send_server_qualification_control(
            private_root,
            nonce,
            sequence=3,
            action="snapshot",
            timeout=timeout,
        )
        write_private_json(
            candidate.resume_path,
            {
                "schema_version": 1,
                "nonce_sha256": candidate.nonce_sha256,
                "correlation_id": candidate.correlation_id,
                "action": "resume_manifest_commit",
            },
        )
        stdout, stderr = candidate.process.communicate(timeout=timeout)
    except BaseException:
        if candidate.process.poll() is None:
            candidate.process.kill()
            stdout, stderr = candidate.process.communicate()
        raise
    finally:
        _restore_fixture(fixture)
    require(
        candidate.process.returncode is not None and candidate.process.returncode != 0,
        "publication candidate did not fail after losing its server lease",
    )
    events = read_jsonl(candidate.event_path)
    require(len(events) == 4, "publication hook did not emit its exact four events")
    return PublicationFaultRun(
        candidate.correlation_id,
        candidate.pause_path,
        candidate.resume_path,
        snapshot_before,
        snapshot_after,
        candidate.process.returncode,
        stdout,
        stderr,
        events,
    )
