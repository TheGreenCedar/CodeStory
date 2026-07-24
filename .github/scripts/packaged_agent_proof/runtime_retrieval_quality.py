"""Retrieval-quality raw evidence verification."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .contract_primitives import require_nonempty_string, require_positive_int
from .foundation import (
    HOLDOUT_TASK_ROOT,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    RELEASE_QUALITY_CORPUS_ID,
    RELEASE_QUALITY_MODES,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    require,
)
from .measurement_protocol import load_holdout_task_contracts
from .runtime_evidence_support import load_external_raw_evidence


@dataclass(frozen=True)
class QualityContract:
    release_evidence: dict
    holdout_tasks: dict
    holdout_manifest_set_sha256: str
    repeats: int
    expected_modes: set[str]
    expected_cells: set[tuple[str, str, str, int]]
    rows: list[dict]


def _quality_contract(
    payload: dict,
    source: dict,
    holdout_task_root: Path,
) -> QualityContract:
    release_evidence = payload.get("release_evidence")
    require(
        isinstance(release_evidence, dict),
        "publishable packet evidence omitted release_evidence",
    )
    for field in ("assertions", "accepted", "decision"):
        require(
            field not in payload and field not in release_evidence,
            f"publishable packet evidence contains self-declared {field}",
        )
    require(
        release_evidence.get("commit") == source["commit"],
        "publishable packet evidence source commit is stale",
    )
    require(
        release_evidence.get("source_tree") == source["tree"],
        "publishable packet evidence source tree is stale",
    )
    require(
        release_evidence.get("evaluation_contract")
        == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "publishable packet evaluation contract is unsupported",
    )
    holdout_tasks, manifest_digest = load_holdout_task_contracts(holdout_task_root)
    evidence_identity = release_evidence.get("evidence_identity")
    require(
        isinstance(evidence_identity, dict)
        and evidence_identity.get("corpus_id") == RELEASE_QUALITY_CORPUS_ID,
        "publishable packet evidence is not bound to the release holdout corpus",
    )
    repeats = require_positive_int(
        release_evidence.get("repeats"),
        "publishable packet repeat count",
    )
    require(
        repeats == MIN_RETRIEVAL_QUALITY_REPEATS,
        f"publishable packet evidence requires exactly {MIN_RETRIEVAL_QUALITY_REPEATS} repeats",
    )
    require(
        release_evidence.get("publishable") is True,
        "packet quality artifact is not publishable",
    )
    require(
        release_evidence.get("quality_gate_status") == "pass",
        "packet quality artifact did not pass its quality gate",
    )
    blockers = release_evidence.get("publishable_blockers")
    require(
        isinstance(blockers, list) and not blockers,
        "packet quality artifact contains publishable blockers",
    )
    require(
        payload.get("repeats") == repeats,
        "packet quality top-level repeat count changed",
    )
    modes = payload.get("modes")
    require(
        isinstance(modes, list) and modes == list(RELEASE_QUALITY_MODES),
        "packet quality artifact must contain only the release cold-cli mode",
    )
    expected_modes = set(RELEASE_QUALITY_MODES.values())
    expected_cells = {
        (repo, task_id, mode, repeat)
        for repo, task_id in holdout_tasks
        for mode in expected_modes
        for repeat in range(1, MIN_RETRIEVAL_QUALITY_REPEATS + 1)
    }
    rows = release_evidence.get("rows")
    require(
        isinstance(rows, list) and rows, "packet quality artifact has no quality rows"
    )
    return QualityContract(
        release_evidence,
        holdout_tasks,
        manifest_digest,
        repeats,
        expected_modes,
        expected_cells,
        rows,
    )


def _verify_row_quality(row: dict, index: int) -> None:
    quality = row.get("quality")
    sufficiency = row.get("sufficiency")
    latency = row.get("packet_latency")
    require(
        row.get("status") == "pass"
        and isinstance(quality, dict)
        and quality.get("pass") is True,
        f"packet quality row {index} did not pass",
    )
    require(
        isinstance(sufficiency, dict)
        and sufficiency.get("status") == "sufficient"
        and sufficiency.get("sufficient_quality_mismatch") is not True,
        f"packet quality row {index} is not sufficient",
    )
    for field in (
        "follow_up_commands_count",
        "open_next_count",
        "gaps_count",
        "coverage_unresolved_blocking_count",
    ):
        value = sufficiency.get(field, 0)
        require(
            isinstance(value, (int, float))
            and not isinstance(value, bool)
            and value == 0,
            f"packet quality row {index} has unresolved {field}",
        )
    require(
        isinstance(latency, dict)
        and latency.get("sla_missed") is False
        and isinstance(latency.get("retrieval_shadow"), dict)
        and latency["retrieval_shadow"].get("retrieval_mode") == "full",
        f"packet quality row {index} lacks full-retrieval latency proof",
    )


def _verified_repository_provenance(row: dict, index: int) -> tuple[dict, dict]:
    provenance = row.get("repo_provenance")
    require(
        isinstance(provenance, dict)
        and provenance.get("manifest_overridden_by_builtin") is False
        and provenance.get("git_dirty") is False,
        f"packet quality row {index} has untrusted repository provenance",
    )
    configured = provenance.get("configured")
    manifest_repo = provenance.get("manifest")
    require(
        isinstance(configured, dict) and isinstance(manifest_repo, dict),
        f"packet quality row {index} omitted repository identities",
    )
    configured_ref = configured.get("ref")
    require(
        isinstance(configured_ref, str)
        and re.fullmatch(r"[0-9a-f]{40}", configured_ref) is not None
        and manifest_repo.get("ref") == configured_ref
        and provenance.get("git_head") == configured_ref,
        f"packet quality row {index} is not pinned to one immutable repository commit",
    )
    trusted_url = re.compile(
        r"^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?$"
    )
    urls = (
        configured.get("url"),
        manifest_repo.get("url"),
        provenance.get("git_origin"),
    )
    require(
        all(isinstance(url, str) and trusted_url.fullmatch(url) for url in urls),
        f"packet quality row {index} has an untrusted repository URL",
    )
    normalized_urls = {
        re.sub(r"\.git$", "", url, flags=re.IGNORECASE).lower() for url in urls
    }
    require(
        len(normalized_urls) == 1,
        f"packet quality row {index} repository URLs disagree",
    )
    return configured, manifest_repo


def _verify_cache_provenance(row: dict, index: int) -> None:
    cache = row.get("codestory_cache_provenance")
    require(
        isinstance(cache, dict)
        and cache.get("doctor_status") == "pass"
        and bool(cache.get("storage_path"))
        and bool(cache.get("cache_policy"))
        and cache.get("cache_policy") != "unprepared-cache-blocked"
        and cache.get("retrieval_mode") == "full"
        and bool(cache.get("semantic_generation"))
        and bool(cache.get("manifest_embedding_backend"))
        and bool(cache.get("embedding_engine_instance_id"))
        and cache.get("embedding_policy") in {"accelerated", "cpu_explicit"}
        and cache.get("semantic_backend") is not None
        and cache.get("local_only") is True
        and cache.get("indexed") is True
        and cache.get("freshness_status") == "fresh"
        and cache.get("semantic_ready") is True
        and cache.get("indexing_in_timed_run") is not None,
        f"packet quality row {index} has incomplete CodeStory cache provenance",
    )


def _quality_cell(
    row: dict,
    *,
    index: int,
    contract: QualityContract,
    configured: dict,
    manifest_repo: dict,
) -> tuple[str, str, str, int]:
    repeat = row.get("repeat")
    require(
        isinstance(repeat, int)
        and not isinstance(repeat, bool)
        and 1 <= repeat <= contract.repeats,
        f"packet quality row {index} has an invalid repeat",
    )
    repo = require_nonempty_string(row.get("repo"), f"packet quality row {index} repo")
    task_id = require_nonempty_string(
        row.get("task_id"), f"packet quality row {index} task id"
    )
    mode = require_nonempty_string(row.get("mode"), f"packet quality row {index} mode")
    require(
        mode in contract.expected_modes,
        f"packet quality row {index} mode is not declared at top level",
    )
    task = contract.holdout_tasks.get((repo, task_id))
    require(
        task is not None,
        f"packet quality row {index} is not one of the checked-in holdout tasks",
    )
    snapshot = row.get("task_manifest_snapshot")
    require(
        isinstance(snapshot, dict),
        f"packet quality row {index} omitted its task manifest snapshot",
    )
    without_path = {
        key: value for key, value in snapshot.items() if key != "manifest_path"
    }
    require(
        without_path == task["snapshot"],
        f"packet quality row {index} task snapshot differs from the checked-in manifest",
    )
    manifest_path = snapshot.get("manifest_path")
    require(
        isinstance(manifest_path, str)
        and Path(manifest_path).name == task["path"].name,
        f"packet quality row {index} names a different task manifest",
    )
    expected_repo = task["repo"]
    require(
        configured.get("url") == expected_repo["url"]
        and configured.get("ref") == expected_repo["ref"]
        and configured.get("languages") == expected_repo.get("languages", [])
        and manifest_repo.get("url") == expected_repo["url"]
        and manifest_repo.get("ref") == expected_repo["ref"]
        and manifest_repo.get("workspace_root") == expected_repo.get("workspace_root"),
        f"packet quality row {index} repository identity differs from its checked-in task",
    )
    return repo, task_id, mode, repeat


def verify_retrieval_quality_raw_evidence(
    path: Path,
    *,
    source: dict,
    holdout_task_root: Path = HOLDOUT_TASK_ROOT,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path,
        "publishable packet quality raw evidence",
    )
    contract = _quality_contract(payload, source, holdout_task_root)
    observed_cells: set[tuple[str, str, str, int]] = set()
    for index, row in enumerate(contract.rows):
        require(isinstance(row, dict), f"packet quality row {index} is malformed")
        _verify_row_quality(row, index)
        configured, manifest_repo = _verified_repository_provenance(row, index)
        _verify_cache_provenance(row, index)
        cell = _quality_cell(
            row,
            index=index,
            contract=contract,
            configured=configured,
            manifest_repo=manifest_repo,
        )
        require(
            cell not in observed_cells,
            f"packet quality rows duplicate repeat {cell[3]} "
            f"for {cell[0]}/{cell[1]}/{cell[2]}",
        )
        observed_cells.add(cell)
    require(
        observed_cells == contract.expected_cells,
        "packet quality rows do not exactly cover the checked-in repo/task/mode/repeat matrix",
    )
    passing_rows = len(contract.rows)
    return {
        "artifact": {
            "name": "packet-runtime-summary.json",
            "sha256": artifact_sha256,
        },
        "evaluation_contract": RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "source_commit": source["commit"],
        "source_tree": source["tree"],
        "corpus_id": RELEASE_QUALITY_CORPUS_ID,
        "holdout_manifest_set_sha256": contract.holdout_manifest_set_sha256,
        "repeats": contract.repeats,
        "row_count": len(contract.rows),
        "passing_row_count": passing_rows,
        "publishable_packet_pass_rate": passing_rows / len(contract.rows),
    }
