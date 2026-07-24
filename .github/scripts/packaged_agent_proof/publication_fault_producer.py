"""Production of publication-fault and rank-consistency evidence."""

from __future__ import annotations

from pathlib import Path

from .contract_primitives import write_private_json
from .publication_fault_evidence import (
    _consistency_payload,
    _post_fault_observations,
    _publication_payload,
)
from .publication_fault_run import _run_fault
from .publication_fault_setup import (
    _baseline_publication,
    _publication_commands,
    _publication_fixture,
)


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
