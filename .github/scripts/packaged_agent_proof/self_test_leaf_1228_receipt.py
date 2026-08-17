"""Focused hostile-mutation tests for the #1228 closure receipt."""

from __future__ import annotations

import copy
import hashlib
import json
import subprocess
import sys
import tempfile
import zipfile
from io import BytesIO
from pathlib import Path

from .leaf_1228_receipt import (
    CANDIDATE_JOB,
    CANDIDATE_STEPS,
    CANDIDATE_WORKFLOW,
    SOURCE_JOB,
    SOURCE_PROOF_SCHEMA,
    SOURCE_STEP,
    SOURCE_WORKFLOW,
    build_issue_1228_receipt,
)


REPOSITORY = "TheGreenCedar/CodeStory"
COMMIT = "a" * 40
TREE = "b" * 40
VERSION = "0.17.0"


def _sha(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _zip(entries: dict[str, bytes]) -> bytes:
    output = BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, payload in entries.items():
            archive.writestr(name, payload)
    return output.getvalue()


def _json(payload: object) -> bytes:
    return (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()


def _run(run_id: int, workflow: str) -> dict:
    return {
        "id": run_id,
        "run_attempt": 1,
        "path": workflow,
        "event": "workflow_dispatch",
        "status": "completed",
        "conclusion": "success",
        "head_sha": COMMIT,
        "head_commit": {"tree_id": TREE},
        "head_repository": {"full_name": REPOSITORY},
        "repository": {"full_name": REPOSITORY},
        "html_url": f"https://github.com/{REPOSITORY}/actions/runs/{run_id}",
    }


def _jobs(run_id: int, name: str, steps: tuple[str, ...]) -> dict:
    return {
        "total_count": 1,
        "jobs": [
            {
                "id": run_id * 10,
                "run_id": run_id,
                "run_attempt": 1,
                "head_sha": COMMIT,
                "name": name,
                "status": "completed",
                "conclusion": "success",
                "steps": [
                    {"name": step, "status": "completed", "conclusion": "success"}
                    for step in steps
                ],
            }
        ],
    }


def _artifact(artifact_id: int, name: str, payload: bytes, run_id: int) -> dict:
    return {
        "id": artifact_id,
        "name": name,
        "size_in_bytes": len(payload),
        "digest": f"sha256:{_sha(payload)}",
        "expired": False,
        "workflow_run": {"id": run_id, "head_sha": COMMIT},
    }


def _write(path: Path, payload: object) -> Path:
    if isinstance(payload, bytes):
        path.write_bytes(payload)
    else:
        path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def _fixture(root: Path, mutation: str | None = None) -> dict:
    source_run_id = 101
    candidate_run_id = 202
    source_run = _run(source_run_id, SOURCE_WORKFLOW)
    source_jobs = _jobs(source_run_id, SOURCE_JOB, (SOURCE_STEP,))
    source_proof = {
        "schema": SOURCE_PROOF_SCHEMA,
        "status": "pass",
        "repository": REPOSITORY,
        "commit": COMMIT,
        "source_tree": TREE,
        "version": VERSION,
        "producer": {
            "workflow_path": SOURCE_WORKFLOW,
            "job": SOURCE_JOB,
            "run_id": source_run_id,
            "run_attempt": 1,
        },
        "contracts": {
            "qualification_harness_self_test": True,
            "control_directory_inflight": True,
            "windows_path_identity": True,
            "native_staging": True,
        },
    }
    candidate_run = _run(candidate_run_id, CANDIDATE_WORKFLOW)
    candidate_jobs = _jobs(candidate_run_id, CANDIDATE_JOB, CANDIDATE_STEPS)

    launcher = b"synthetic PE launcher"
    runtime = b"synthetic PE managed runtime"
    module = b"synthetic Vulkan module"
    generation = "c" * 64
    manifest = {
        "schema_version": 3,
        "release_version": VERSION,
        "asset_target": "windows-x64",
        "source": {"commit": COMMIT, "tree": TREE, "tracked_dirty": False},
        "binary": {"name": "codestory-cli.exe", "sha256": _sha(launcher)},
        "runtime_executable": {
            "name": "codestory-native-runtime.exe",
            "sha256": _sha(runtime),
            "generation_id": generation,
        },
        "runtime_artifacts": [{"name": "ggml-vulkan.dll", "sha256": _sha(module)}],
    }
    root_name = f"codestory-cli-v{VERSION}-windows-x64"
    runtime_root = f"{root_name}/codestory-native-generations/{generation}"
    archive_entries = {
        f"{root_name}/codestory-cli.exe": launcher,
        f"{root_name}/codestory-native-manifest.json": _json(manifest),
        f"{root_name}/codestory-native-current-generation-v1.txt": f"{generation}\n".encode(),
        f"{runtime_root}/codestory-native-runtime.exe": runtime,
        f"{runtime_root}/ggml-vulkan.dll": module,
    }
    if mutation == "runtime_bytes":
        archive_entries[f"{runtime_root}/codestory-native-runtime.exe"] += b" hostile"
    archive_payload = _zip(archive_entries)
    archive_name = f"{root_name}.zip"
    checksum = f"{_sha(archive_payload)}  {archive_name}\n".encode()
    package_container = _zip(
        {archive_name: archive_payload, f"{archive_name}.sha256": checksum, "SHA256SUMS.txt": checksum}
    )
    record = {
        "schema": "codestory-candidate-archive-store/v1",
        "repository": REPOSITORY,
        "source": {"commit": COMMIT, "tree": TREE},
        "target": "windows-x64",
        "archive": {
            "name": archive_name,
            "relative_path": archive_name,
            "bytes": len(archive_payload),
            "sha256": _sha(archive_payload),
        },
        "companions": [
            {
                "role": "archive_checksum",
                "relative_path": f"{archive_name}.sha256",
                "bytes": len(checksum),
                "sha256": _sha(checksum),
            },
            {
                "role": "checksum_manifest",
                "relative_path": "SHA256SUMS.txt",
                "bytes": len(checksum),
                "sha256": _sha(checksum),
            },
        ],
    }
    if mutation == "record_archive_digest":
        record["archive"]["sha256"] = "d" * 64
    record_container = _zip({"candidate-archive-record.json": _json(record)})
    summary_manifest = copy.deepcopy(manifest)
    if mutation == "version_manifest":
        summary_manifest["source"]["tree"] = "e" * 40
    version_container = _zip({"summary.json": _json({"package_contract": {"manifest": summary_manifest}})})
    if mutation == "source_contract":
        source_proof["contracts"]["native_staging"] = False
    source_container = _zip({"windows-native-source-proof.json": _json(source_proof)})

    source_artifacts = {
        "total_count": 1,
        "artifacts": [
            _artifact(
                1001,
                f"windows-native-source-proof-{COMMIT}-attempt-1",
                source_container,
                source_run_id,
            )
        ],
    }
    candidate_artifacts = {
        "total_count": 3,
        "artifacts": [
            _artifact(2001, "codestory-cli-windows-x64", package_container, candidate_run_id),
            _artifact(
                2002,
                "codestory-candidate-archive-record-windows-x64",
                record_container,
                candidate_run_id,
            ),
            _artifact(
                2003,
                "packaged-version-proof-windows-x64-attempt-1",
                version_container,
                candidate_run_id,
            ),
        ],
    }
    if mutation == "source_step":
        source_jobs["jobs"][0]["steps"][0]["conclusion"] = "failure"
    if mutation == "candidate_tree":
        candidate_run["head_commit"]["tree_id"] = "f" * 40
    if mutation == "source_artifact_digest":
        source_artifacts["artifacts"][0]["digest"] = "sha256:" + "0" * 64
    if mutation == "package_artifact_digest":
        candidate_artifacts["artifacts"][0]["digest"] = "sha256:" + "0" * 64

    return {
        "repository": REPOSITORY,
        "candidate_sha": COMMIT,
        "candidate_tree": TREE,
        "version": VERSION,
        "source_run_json": _write(root / "source-run.json", source_run),
        "source_jobs_json": _write(root / "source-jobs.json", source_jobs),
        "source_artifacts_json": _write(root / "source-artifacts.json", source_artifacts),
        "source_proof_artifact_container": _write(root / "source-proof.zip", source_container),
        "candidate_run_json": _write(root / "candidate-run.json", candidate_run),
        "candidate_jobs_json": _write(root / "candidate-jobs.json", candidate_jobs),
        "candidate_artifacts_json": _write(root / "candidate-artifacts.json", candidate_artifacts),
        "package_artifact_container": _write(root / "package.zip", package_container),
        "archive_record_artifact_container": _write(root / "record.zip", record_container),
        "version_proof_artifact_container": _write(root / "version.zip", version_container),
    }


def _decision(mutation: str | None = None) -> dict:
    with tempfile.TemporaryDirectory(prefix="codestory-leaf-1228-") as raw:
        return build_issue_1228_receipt(**_fixture(Path(raw), mutation))


def run_issue_1228_receipt_self_tests() -> None:
    accepted = _decision()
    assert accepted["decision"] == "accept", accepted
    assert accepted == _decision(), "receipt changed across identical evidence"
    assert accepted["evidence"]["source_contracts"]["native_staging"] is True
    assert accepted["evidence"]["native"]["final_entries_are_distinct_regular_files"] is True
    for mutation in (
        "source_contract",
        "source_step",
        "source_artifact_digest",
        "candidate_tree",
        "record_archive_digest",
        "runtime_bytes",
        "version_manifest",
        "package_artifact_digest",
    ):
        rejected = _decision(mutation)
        assert rejected["decision"] == "reject", (mutation, rejected)
        assert rejected["evidence"] == {}, mutation
        assert rejected["rejection"]["code"] == "contract_rejected", mutation
    with tempfile.TemporaryDirectory(prefix="codestory-leaf-1228-cli-") as raw:
        root = Path(raw)
        arguments = _fixture(root)
        output = root / "receipt.json"
        command = [sys.executable, str(Path(__file__).parents[1] / "check-v017-issue-1228-artifacts.py")]
        for name, value in arguments.items():
            command.extend((f"--{name.replace('_', '-')}", str(value)))
        command.extend(("--out", str(output)))
        completed = subprocess.run(command, check=False, capture_output=True, text=True)
        assert completed.returncode == 0, completed.stderr
        assert json.loads(output.read_text(encoding="utf-8"))["decision"] == "accept"
    with tempfile.TemporaryDirectory(prefix="codestory-leaf-1228-cli-reject-") as raw:
        root = Path(raw)
        arguments = _fixture(root, "source_contract")
        output = root / "receipt.json"
        command = [sys.executable, str(Path(__file__).parents[1] / "check-v017-issue-1228-artifacts.py")]
        for name, value in arguments.items():
            command.extend((f"--{name.replace('_', '-')}", str(value)))
        command.extend(("--out", str(output)))
        completed = subprocess.run(command, check=False, capture_output=True, text=True)
        assert completed.returncode == 1, completed.stderr
        assert json.loads(output.read_text(encoding="utf-8"))["decision"] == "reject"
    print("issue #1228 artifact receipt self-test passed (2 accepts, 9 hostile rejects)")


if __name__ == "__main__":
    run_issue_1228_receipt_self_tests()
