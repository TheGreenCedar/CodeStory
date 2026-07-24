"""Production of publication-fault and rank-consistency evidence."""

from __future__ import annotations

import hashlib
import os
import secrets
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .contracts import write_private_json
from .foundation import (
    FAULT_RECOVERY_CONSISTENCY_CASES,
    FAULT_RECOVERY_CONSISTENCY_CONTRACT,
    PUBLICATION_FAULT_EVIDENCE_CONTRACT,
    require,
)
from .process import json_command
from .publication_protocol import (
    publication_identity_from_status,
    read_jsonl,
    run_publication_replacement_worker,
    run_quality_search,
    send_server_qualification_control,
    server_observation_from_control_event,
    wait_for_jsonl_event,
)


@dataclass(frozen=True)
class PublicationFixture:
    project: Path
    anchors: list[str]
    source_file: Path
    lexical_file: Path
    baseline_source: str
    baseline_lexical: str
    file_times: dict[Path, tuple[int, int]]


@dataclass(frozen=True)
class PublicationCommands:
    run_id: str
    index: list[str]
    retrieval_index: list[str]
    status: list[str]


@dataclass(frozen=True)
class PublicationFaultRun:
    correlation_id: str
    pause_path: Path
    resume_path: Path
    snapshot_before: dict
    snapshot_after: dict
    returncode: int
    stdout: str
    stderr: str
    hook_events: list[dict]


@dataclass(frozen=True)
class PublicationCandidate:
    correlation_id: str
    nonce_sha256: str
    pause_path: Path
    resume_path: Path
    event_path: Path
    process: subprocess.Popen


def _publication_fixture(private_root: Path) -> PublicationFixture:
    project = private_root / "publication-product-repository"
    project.mkdir(mode=0o700)
    anchors = [
        f"qualification_anchor_{index:02d}"
        for index in range(FAULT_RECOVERY_CONSISTENCY_CASES)
    ]
    source_file = project / "lib.rs"
    baseline_source = (
        "\n".join(
            f'pub fn {anchor}() -> &\'static str {{ "{anchor}" }}'
            for anchor in anchors
        )
        + "\n"
    )
    source_file.write_text(baseline_source, encoding="utf-8")
    lexical_file = project / "README.md"
    baseline_lexical = "# Publication qualification baseline\n"
    lexical_file.write_text(baseline_lexical, encoding="utf-8")
    file_times = {
        path: (metadata.st_atime_ns, metadata.st_mtime_ns)
        for path in (source_file, lexical_file)
        for metadata in (path.stat(),)
    }
    return PublicationFixture(
        project,
        anchors,
        source_file,
        lexical_file,
        baseline_source,
        baseline_lexical,
        file_times,
    )


def _publication_commands(cli: Path, project: Path) -> PublicationCommands:
    run_id = "publication-qualification"
    return PublicationCommands(
        run_id=run_id,
        index=[
            str(cli),
            "index",
            "--project",
            str(project),
            "--refresh",
            "full",
            "--format",
            "json",
        ],
        retrieval_index=[
            str(cli),
            "retrieval",
            "index",
            "--project",
            str(project),
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
        status=[
            str(cli),
            "retrieval",
            "status",
            "--project",
            str(project),
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
    )


def _baseline_publication(
    cli: Path,
    env: dict[str, str],
    fixture: PublicationFixture,
    commands: PublicationCommands,
    *,
    timeout: int,
) -> tuple[str, list[int | None]]:
    json_command(commands.index, env=env, cwd=fixture.project, timeout=timeout)
    json_command(
        commands.retrieval_index,
        env=env,
        cwd=fixture.project,
        timeout=timeout,
    )
    _, status = json_command(
        commands.status,
        env=env,
        cwd=fixture.project,
        timeout=timeout,
    )
    publication = publication_identity_from_status(status)
    ranks = [
        run_quality_search(
            cli,
            env,
            fixture.project,
            commands.run_id,
            anchor,
            anchor,
            timeout=timeout,
        )[0]
        for anchor in fixture.anchors
    ]
    return publication, ranks


def _restore_fixture(fixture: PublicationFixture) -> None:
    fixture.source_file.write_text(fixture.baseline_source, encoding="utf-8")
    fixture.lexical_file.write_text(fixture.baseline_lexical, encoding="utf-8")
    for path, times in fixture.file_times.items():
        os.utime(path, ns=times)


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
            lambda event: event.get("action") == "pause_before_manifest_commit"
            and event.get("status") == "waiting_for_resume",
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


def _post_fault_observations(
    cli: Path,
    env: dict[str, str],
    fixture: PublicationFixture,
    commands: PublicationCommands,
    *,
    timeout: int,
) -> tuple[dict, str, list[int | None], str, str]:
    status_result, status = json_command(
        commands.status,
        env=env,
        cwd=fixture.project,
        timeout=timeout,
    )
    publication = publication_identity_from_status(status)
    ranks = []
    first_search_sha256 = None
    for anchor in fixture.anchors:
        rank, output_sha256 = run_quality_search(
            cli,
            env,
            fixture.project,
            commands.run_id,
            anchor,
            anchor,
            timeout=timeout,
        )
        ranks.append(rank)
        if first_search_sha256 is None:
            first_search_sha256 = output_sha256
    require(first_search_sha256 is not None, "qualification search emitted no output digest")
    status_sha256 = hashlib.sha256(
        status_result["stdout"].encode("utf-8")
    ).hexdigest()
    return status, publication, ranks, status_sha256, first_search_sha256


def _publication_payload(
    fault: PublicationFaultRun,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    previous_publication: str,
    final_status: dict,
    final_publication: str,
    status_sha256: str,
    search_sha256: str,
) -> dict:
    return {
        "schema_version": 1,
        "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "correlation_id": fault.correlation_id,
        "previous_publication_identity_sha256": previous_publication,
        "server_observations": [
            server_observation_from_control_event(
                fault.snapshot_before,
                "before_crash",
            ),
            server_observation_from_control_event(
                fault.snapshot_after,
                "after_replacement",
            ),
        ],
        "candidate_observation": {
            "command": "retrieval_index",
            "exit_code": fault.returncode,
            "stdout_sha256": hashlib.sha256(fault.stdout.encode("utf-8")).hexdigest(),
            "stderr_sha256": hashlib.sha256(fault.stderr.encode("utf-8")).hexdigest(),
        },
        "publication_hook_events": fault.hook_events,
        "ordinary_product_observations": [
            {
                "sequence": 0,
                "command": "retrieval_status",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": status_sha256,
            },
            {
                "sequence": 1,
                "command": "search",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": search_sha256,
            },
        ],
    }


def _consistency_payload(
    fixture: PublicationFixture,
    fault: PublicationFaultRun,
    baseline_ranks: list[int | None],
    post_ranks: list[int | None],
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    return {
        "schema_version": 1,
        "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "run_id_sha256": hashlib.sha256(
            fault.correlation_id.encode("ascii")
        ).hexdigest(),
        "observations": [
            {
                "case_id_sha256": hashlib.sha256(anchor.encode("utf-8")).hexdigest(),
                "before_server_fault_rank": baseline_ranks[index],
                "after_server_replacement_rank": post_ranks[index],
            }
            for index, anchor in enumerate(fixture.anchors)
        ],
    }


def produce_product_publication_fault_evidence(
    cli: Path,
    env: dict[str, str],
    private_root: Path,
    artifact_root: Path,
    nonce: str,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    timeout: int,
) -> tuple[Path, Path]:
    fixture = _publication_fixture(private_root)
    commands = _publication_commands(cli, fixture.project)
    previous_publication, baseline_ranks = _baseline_publication(
        cli,
        env,
        fixture,
        commands,
        timeout=timeout,
    )
    fault = _run_fault(
        cli,
        env,
        private_root,
        nonce,
        fixture,
        commands,
        timeout=timeout,
    )
    final_status, final_publication, post_ranks, status_sha256, search_sha256 = (
        _post_fault_observations(
            cli,
            env,
            fixture,
            commands,
            timeout=timeout,
        )
    )
    publication_path = artifact_root / "publication-fault-external.raw.json"
    write_private_json(
        publication_path,
        _publication_payload(
            fault,
            source=source,
            package=package,
            contracts=contracts,
            previous_publication=previous_publication,
            final_status=final_status,
            final_publication=final_publication,
            status_sha256=status_sha256,
            search_sha256=search_sha256,
        ),
    )
    consistency_path = artifact_root / "fault-recovery-consistency.raw.json"
    write_private_json(
        consistency_path,
        _consistency_payload(
            fixture,
            fault,
            baseline_ranks,
            post_ranks,
            source=source,
            package=package,
            contracts=contracts,
        ),
    )
    for path in (fault.pause_path, fault.resume_path):
        path.unlink(missing_ok=True)
    return publication_path, consistency_path
