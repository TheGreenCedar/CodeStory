"""Publication-fault fixture and baseline setup."""

from __future__ import annotations

import os
from pathlib import Path

from .foundation import FAULT_RECOVERY_CONSISTENCY_CASES
from .process import json_command
from .publication_fault_types import PublicationCommands, PublicationFixture
from .publication_protocol import publication_identity_from_status, run_quality_search


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
            f'pub fn {anchor}() -> &\'static str {{ "{anchor}" }}' for anchor in anchors
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
