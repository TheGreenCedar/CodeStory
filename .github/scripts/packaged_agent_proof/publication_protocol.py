"""Publication qualification protocol operations."""

from __future__ import annotations

import hashlib
import json
import subprocess
import time
from pathlib import Path

from .contract_primitives import (
    canonical_sha256,
    require_nonempty_string,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    sha256,
    write_private_json,
)
from .foundation import ProofFailure, require
from .subprocess_control import json_command, run


def read_jsonl(path: Path) -> list[dict]:
    if not path.is_file() or path.is_symlink():
        return []
    events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def wait_for_jsonl_event(
    path: Path,
    predicate,
    *,
    timeout: int,
    process: subprocess.Popen | None = None,
) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for event in read_jsonl(path):
            if predicate(event):
                return event
        if process is not None and process.poll() is not None:
            stdout, stderr = process.communicate()
            raise ProofFailure(
                "qualification product process exited before its raw event: "
                f"exit={process.returncode} stdout_sha256="
                f"{hashlib.sha256(stdout.encode('utf-8')).hexdigest()} stderr_sha256="
                f"{hashlib.sha256(stderr.encode('utf-8')).hexdigest()}"
            )
        time.sleep(0.01)
    raise ProofFailure(f"timed out waiting for qualification event file {path.name}")


def send_server_qualification_control(
    directory: Path,
    nonce: str,
    *,
    sequence: int,
    action: str,
    timeout: int,
) -> dict:
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    command_path = directory / f"{nonce}.command.json"
    require(
        not command_path.exists(), "stale embedding qualification command is present"
    )
    write_private_json(
        command_path,
        {
            "schema_version": 1,
            "sequence": sequence,
            "nonce_sha256": nonce_sha256,
            "action": action,
            "parameters": {"class": None},
        },
    )
    try:
        event_path = directory / f"{nonce}.events.jsonl"
        event = wait_for_jsonl_event(
            event_path,
            lambda candidate: (
                candidate.get("sequence") == sequence
                and candidate.get("action") == action
            ),
            timeout=timeout,
        )
        require(
            event.get("status") in {"completed", "accepted"},
            f"embedding qualification control {action} failed",
        )
        return event
    finally:
        command_path.unlink(missing_ok=True)


def server_observation_from_control_event(event: dict, phase: str) -> dict:
    snapshot = event.get("snapshot")
    require(
        isinstance(snapshot, dict), f"{phase} control event omitted its server snapshot"
    )
    process = snapshot.get("process")
    engine = snapshot.get("engine")
    require(
        isinstance(process, dict), f"{phase} server snapshot omitted process identity"
    )
    require(
        isinstance(engine, dict),
        f"{phase} server snapshot omitted resident engine identity",
    )
    process_start = require_nonempty_string(
        process.get("process_start_id"), f"{phase} process start"
    )
    return {
        "phase": phase,
        "server_instance_id": require_opaque_identifier(
            process.get("server_instance_id"), f"{phase} server instance"
        ),
        "process_start_id": hashlib.sha256(process_start.encode("utf-8")).hexdigest(),
        "load_generation": require_positive_int(
            engine.get("load_generation"), f"{phase} load generation"
        ),
    }


def publication_identity_from_status(status: dict) -> str:
    require(status.get("retrieval_mode") == "full", "qualification status is not full")
    contract = status.get("manifest_contract")
    manifest = status.get("manifest")
    require(
        isinstance(contract, dict),
        "qualification status omitted its manifest contract",
    )
    require(
        isinstance(manifest, dict),
        "qualification status omitted its published manifest",
    )
    generation = require_nonempty_string(
        contract.get("generation"), "qualification manifest contract generation"
    )
    input_hash = require_sha256(
        contract.get("input_hash"), "qualification manifest contract input hash"
    )
    project_id = require_opaque_identifier(
        contract.get("project_id"), "qualification manifest contract project"
    )
    schema_version = require_positive_int(
        contract.get("schema_version"), "qualification manifest contract schema"
    )
    graph_hash = require_sha256(
        contract.get("graph_hash"), "qualification manifest contract graph hash"
    )
    require(
        manifest.get("project_id") == project_id
        and manifest.get("sidecar_generation") == generation
        and manifest.get("sidecar_input_hash") == input_hash
        and manifest.get("sidecar_schema_version") == schema_version
        and manifest.get("graph_artifact_hash") == graph_hash,
        "qualification manifest report disagrees with its manifest contract",
    )
    return canonical_sha256(
        {
            "project_id": project_id,
            "generation": generation,
            "input_hash": input_hash,
            "schema_version": schema_version,
            "graph_hash": graph_hash,
            "lexical_version": require_nonempty_string(
                manifest.get("lexical_version"),
                "qualification manifest lexical version",
            ),
            "semantic_generation": require_nonempty_string(
                manifest.get("semantic_generation"),
                "qualification manifest semantic generation",
            ),
            "scip_revision": require_nonempty_string(
                manifest.get("scip_revision"),
                "qualification manifest SCIP revision",
            ),
        }
    )


def run_quality_search(
    cli: Path,
    env: dict[str, str],
    project: Path,
    run_id: str,
    query: str,
    expected: str,
    *,
    timeout: int,
) -> tuple[int | None, str]:
    result, payload = json_command(
        [
            str(cli),
            "search",
            "--project",
            str(project),
            "--query",
            query,
            "--limit",
            "10",
            "--repo-text",
            "off",
            "--refresh",
            "none",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    hits = payload.get("indexed_symbol_hits")
    require(isinstance(hits, list), "qualification search omitted indexed symbol hits")
    position = next(
        (
            index
            for index, hit in enumerate(hits)
            if isinstance(hit, dict)
            and isinstance(hit.get("display_name"), str)
            and expected in hit["display_name"]
        ),
        None,
    )
    rank = None if position is None or position >= 10 else position + 1
    output_sha256 = hashlib.sha256(result["stdout"].encode("utf-8")).hexdigest()
    return rank, output_sha256


def run_publication_replacement_worker(
    cli: Path,
    env: dict[str, str],
    project: Path,
    private_root: Path,
    nonce: str,
    *,
    timeout: int,
) -> None:
    request_path = private_root / "publication-replacement-worker-request.json"
    output_path = private_root / "publication-replacement-worker-output.json"
    write_private_json(
        request_path,
        {
            "schema_version": 1,
            "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
            "executable_sha256": sha256(cli),
            "project": str(project.resolve()),
            "operation": "query",
            "parameters": {
                "query_count": 1,
                "bulk_count": 0,
                "documents_per_bulk": 0,
                "input_bytes": 64,
                "hold_ms": 0,
            },
        },
    )
    run(
        [
            str(cli),
            "internal-embedding-qualification-worker",
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    require(
        output_path.is_file() and not output_path.is_symlink(),
        "publication replacement worker omitted its output",
    )
    try:
        output = json.loads(output_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"publication replacement worker output is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(output, dict)
        and output.get("schema_version") == 1
        and output.get("executable_sha256") == sha256(cli)
        and output.get("error") is None,
        "publication replacement worker failed",
    )
    result = output.get("result")
    operations = result.get("operations") if isinstance(result, dict) else None
    require(
        isinstance(result, dict)
        and result.get("schema_version") == 1
        and result.get("scenario") == "query"
        and isinstance(operations, list)
        and len(operations) == 1
        and operations[0].get("status") == "ok"
        and operations[0].get("error_code") is None,
        "publication replacement worker did not complete its query",
    )
