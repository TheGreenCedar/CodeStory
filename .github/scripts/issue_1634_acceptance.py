#!/usr/bin/env python3
"""Authenticate the exact source and Windows qualification artifacts for #1634."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import stat
import sys
import zipfile
from pathlib import Path, PurePosixPath


ACCEPTANCE_SCHEMA = "codestory.issue-1634-acceptance/v1"
SOURCE_RECEIPT_SCHEMA = "codestory.windows-native-source-proof/v1"
SOURCE_WORKFLOW = ".github/workflows/source-proof.yml"
SOURCE_JOB = "windows-native-contracts"
SOURCE_STEPS = (
    "Prove the Windows-native qualification harness contracts",
    "Prove Windows path and native-staging source contracts",
    "Emit authenticated Windows-native source receipt",
)
QUALIFICATION_WORKFLOW = ".github/workflows/packaged-platform-pr.yml"
QUALIFICATION_JOB = "windows-vulkan-proof / Packaged Windows Vulkan engine"
QUALIFICATION_STEPS = (
    "Prove protected Windows Vulkan runtime",
    "Upload Vulkan proof artifacts",
)
SHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")


class AcceptanceRejection(ValueError):
    """The supplied artifacts do not prove issue #1634 acceptance."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise AcceptanceRejection(message)


def _object(value: object, label: str) -> dict:
    _require(isinstance(value, dict), f"{label} must be an object")
    return value


def _list(value: object, label: str) -> list:
    _require(isinstance(value, list), f"{label} must be an array")
    return value


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _load_json(path: Path, label: str) -> dict:
    try:
        return _object(json.loads(path.read_text(encoding="utf-8")), label)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise AcceptanceRejection(f"{label} is unreadable JSON: {exc}") from exc


def _workflow_run(
    document: dict,
    *,
    label: str,
    workflow: str,
    repository: str,
    commit: str,
    tree: str,
) -> dict:
    _require(document.get("path") == workflow, f"{label} workflow path changed")
    _require(document.get("event") == "workflow_dispatch", f"{label} was not dispatched")
    _require(document.get("status") == "completed", f"{label} is not complete")
    _require(document.get("conclusion") == "success", f"{label} did not succeed")
    _require(document.get("head_sha") == commit, f"{label} commit is not exact")
    _require(
        _object(document.get("repository"), f"{label} repository").get("full_name")
        == repository,
        f"{label} API repository changed",
    )
    _require(
        _object(document.get("head_repository"), f"{label} head repository").get(
            "full_name"
        )
        == repository,
        f"{label} came from a different repository",
    )
    _require(
        _object(document.get("head_commit"), f"{label} head commit").get("tree_id")
        == tree,
        f"{label} tree is not exact",
    )
    run_id = document.get("id")
    attempt = document.get("run_attempt")
    _require(isinstance(run_id, int) and run_id > 0, f"{label} run id is invalid")
    _require(isinstance(attempt, int) and attempt > 0, f"{label} attempt is invalid")
    url = f"https://github.com/{repository}/actions/runs/{run_id}"
    _require(document.get("html_url") == url, f"{label} URL changed")
    return {"id": run_id, "attempt": attempt, "url": url, "workflow": workflow}


def _successful_job(
    document: dict,
    *,
    run: dict,
    commit: str,
    name: str,
    required_steps: tuple[str, ...],
) -> dict:
    jobs = _list(document.get("jobs"), "jobs response")
    _require(document.get("total_count") == len(jobs), "jobs response is truncated")
    matches = [job for job in jobs if isinstance(job, dict) and job.get("name") == name]
    _require(len(matches) == 1, f"expected exactly one {name!r} job")
    job = matches[0]
    _require(job.get("run_id") == run["id"], f"{name} belongs to a different run")
    _require(
        job.get("run_attempt") == run["attempt"],
        f"{name} belongs to a different attempt",
    )
    _require(job.get("head_sha") == commit, f"{name} commit is not exact")
    _require(job.get("status") == "completed", f"{name} is not complete")
    _require(job.get("conclusion") == "success", f"{name} did not succeed")
    steps = _list(job.get("steps"), f"{name} steps")
    for required in required_steps:
        matched = [
            step
            for step in steps
            if isinstance(step, dict) and step.get("name") == required
        ]
        _require(len(matched) == 1, f"{name} is missing exact step {required!r}")
        _require(matched[0].get("conclusion") == "success", f"{required} did not succeed")
    job_id = job.get("id")
    _require(isinstance(job_id, int) and job_id > 0, f"{name} job id is invalid")
    return {"id": job_id, "name": name, "steps": list(required_steps)}


def _safe_zip(payload: bytes, label: str) -> dict[str, bytes]:
    entries: dict[str, bytes] = {}
    folded: set[str] = set()
    try:
        with zipfile.ZipFile(io.BytesIO(payload)) as archive:
            for info in archive.infolist():
                item = PurePosixPath(info.filename)
                _require(
                    bool(info.filename)
                    and "\\" not in info.filename
                    and "\0" not in info.filename
                    and not item.is_absolute()
                    and all(part not in ("", ".", "..") for part in item.parts),
                    f"{label} contains an unsafe path",
                )
                mode = info.external_attr >> 16
                file_type = stat.S_IFMT(mode)
                _require(
                    not file_type or file_type in (stat.S_IFREG, stat.S_IFDIR),
                    f"{label} contains a non-regular entry",
                )
                if info.is_dir():
                    continue
                folded_name = info.filename.casefold()
                _require(folded_name not in folded, f"{label} has duplicate Windows paths")
                _require(not (info.flag_bits & 1), f"{label} contains an encrypted entry")
                folded.add(folded_name)
                entries[info.filename] = archive.read(info)
    except (OSError, zipfile.BadZipFile, RuntimeError) as exc:
        raise AcceptanceRejection(f"{label} is not a readable ZIP: {exc}") from exc
    _require(bool(entries), f"{label} is empty")
    return entries


def _artifact(
    response: dict,
    *,
    name: str,
    run: dict,
    commit: str,
    container: Path,
) -> tuple[dict, dict[str, bytes]]:
    artifacts = _list(response.get("artifacts"), "artifacts response")
    _require(
        response.get("total_count") == len(artifacts),
        "artifacts response is truncated",
    )
    matches = [item for item in artifacts if isinstance(item, dict) and item.get("name") == name]
    _require(len(matches) == 1, f"expected exactly one {name!r} artifact")
    item = matches[0]
    _require(item.get("expired") is False, f"{name} artifact expired")
    workflow_run = _object(item.get("workflow_run"), f"{name} workflow run")
    _require(workflow_run.get("id") == run["id"], f"{name} belongs to a different run")
    _require(workflow_run.get("head_sha") == commit, f"{name} commit is not exact")
    artifact_id = item.get("id")
    size = item.get("size_in_bytes")
    _require(isinstance(artifact_id, int) and artifact_id > 0, f"{name} id is invalid")
    _require(isinstance(size, int) and size > 0, f"{name} size is invalid")
    try:
        payload = container.read_bytes()
    except OSError as exc:
        raise AcceptanceRejection(f"{name} container is unreadable: {exc}") from exc
    _require(len(payload) == size, f"{name} container size differs from GitHub metadata")
    metadata_digest = item.get("digest")
    _require(
        isinstance(metadata_digest, str) and metadata_digest.startswith("sha256:"),
        f"{name} has no GitHub SHA-256 digest",
    )
    digest = metadata_digest[7:]
    _require(SHA256.fullmatch(digest) is not None, f"{name} GitHub digest is invalid")
    _require(_sha256(payload) == digest, f"{name} container digest differs from GitHub metadata")
    return (
        {"id": artifact_id, "name": name, "bytes": size, "sha256": digest},
        _safe_zip(payload, f"{name} artifact container"),
    )


def _one_basename(entries: dict[str, bytes], basename: str, label: str) -> bytes:
    matches = [
        payload
        for name, payload in entries.items()
        if PurePosixPath(name).name == basename
    ]
    _require(len(matches) == 1, f"{label} must contain exactly one {basename}")
    return matches[0]


def _json_payload(payload: bytes, label: str) -> dict:
    try:
        return _object(json.loads(payload.decode("utf-8")), label)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise AcceptanceRejection(f"{label} is not valid JSON: {exc}") from exc


def _source_receipt(
    document: dict,
    *,
    repository: str,
    commit: str,
    tree: str,
    version: str,
    run: dict,
) -> dict:
    _require(
        set(document)
        == {
            "schema",
            "status",
            "repository",
            "commit",
            "source_tree",
            "version",
            "producer",
            "contracts",
        },
        "Windows-native source receipt keys changed",
    )
    _require(document.get("schema") == SOURCE_RECEIPT_SCHEMA, "source receipt schema changed")
    _require(document.get("status") == "pass", "source receipt did not pass")
    _require(document.get("repository") == repository, "source receipt repository changed")
    _require(document.get("commit") == commit, "source receipt commit is not exact")
    _require(document.get("source_tree") == tree, "source receipt tree is not exact")
    _require(document.get("version") == version, "source receipt version changed")
    _require(
        document.get("producer")
        == {
            "workflow_path": SOURCE_WORKFLOW,
            "job": SOURCE_JOB,
            "run_id": run["id"],
            "run_attempt": run["attempt"],
        },
        "source receipt producer run attempt or job changed",
    )
    _require(
        document.get("contracts")
        == {
            "qualification_harness_self_test": True,
            "control_directory_inflight": True,
            "windows_path_identity": True,
            "native_staging": True,
        },
        "source control-directory/inflight contract marker is missing",
    )
    return document


def _qualification(
    document: dict,
    entries: dict[str, bytes],
    *,
    commit: str,
    tree: str,
    version: str,
) -> dict:
    _require(document.get("schema_version") == 1, "qualification schema changed")
    _require(document.get("status") == "pass", "qualification status is not pass")
    source = _object(document.get("source"), "qualification source")
    package = _object(document.get("package"), "qualification package")
    _require(source.get("commit") == commit, "qualification commit is not exact")
    _require(source.get("tree") == tree, "qualification tree is not exact")
    _require(package.get("release_version") == version, "qualification version changed")
    scenario = _object(
        _object(document.get("scenarios"), "qualification scenarios").get("server_crash"),
        "qualification server_crash scenario",
    )
    _require(scenario.get("status") == "pass", "qualification server_crash status is not pass")
    assertions = _object(scenario.get("assertions"), "server_crash assertions")
    _require(bool(assertions), "server_crash assertions are empty")
    _require(
        all(value is True for value in assertions.values()),
        "every server_crash assertion must be true",
    )
    references = _list(scenario.get("artifacts"), "server_crash artifact references")
    _require(bool(references), "server_crash retained artifact references are empty")
    retained = []
    for index, reference in enumerate(references):
        item = _object(reference, f"server_crash artifact reference {index}")
        _require(set(item) == {"name", "sha256"}, f"server_crash artifact reference {index} changed")
        name = item.get("name")
        digest = item.get("sha256")
        _require(
            isinstance(name, str)
            and PurePosixPath(name).name == name
            and name not in ("", ".", ".."),
            f"server_crash artifact reference {index} name is unsafe",
        )
        _require(isinstance(digest, str) and SHA256.fullmatch(digest), f"{name} digest is invalid")
        payload = _one_basename(entries, name, "qualification artifact container")
        _require(_sha256(payload) == digest, f"{name} retained digest differs from its payload")
        retained.append({"name": name, "sha256": digest})
    return {
        "status": "pass",
        "scenario": "server_crash",
        "assertions": sorted(assertions),
        "retained_artifacts": retained,
    }


def _reject(repository: str, commit: str, tree: str, version: str, reason: str) -> dict:
    return {
        "schema": ACCEPTANCE_SCHEMA,
        "decision": "reject",
        "repository": repository,
        "commit": commit,
        "source_tree": tree,
        "version": version,
        "source_proof": None,
        "qualification": None,
        "reason": reason,
    }


def issue_1634_acceptance(
    *,
    repository: str,
    commit: str,
    tree: str,
    version: str,
    source_run_json: Path,
    source_jobs_json: Path,
    source_artifacts_json: Path,
    source_artifact_zip: Path,
    qualification_run_json: Path,
    qualification_jobs_json: Path,
    qualification_artifacts_json: Path,
    qualification_artifact_zip: Path,
) -> dict:
    """Return a deterministic accept/reject receipt for the supplied artifacts."""
    try:
        _require(repository.count("/") == 1, "repository must use owner/name")
        _require(SHA.fullmatch(commit) is not None, "commit must be a full lowercase Git SHA")
        _require(SHA.fullmatch(tree) is not None, "tree must be a full lowercase Git SHA")
        _require(VERSION.fullmatch(version) is not None, "version must be plain semver")

        source_run = _workflow_run(
            _load_json(source_run_json, "source run"),
            label="source run",
            workflow=SOURCE_WORKFLOW,
            repository=repository,
            commit=commit,
            tree=tree,
        )
        source_job = _successful_job(
            _load_json(source_jobs_json, "source jobs"),
            run=source_run,
            commit=commit,
            name=SOURCE_JOB,
            required_steps=SOURCE_STEPS,
        )
        source_name = f"windows-native-source-proof-{commit}-attempt-{source_run['attempt']}"
        source_container, source_entries = _artifact(
            _load_json(source_artifacts_json, "source artifacts"),
            name=source_name,
            run=source_run,
            commit=commit,
            container=source_artifact_zip,
        )
        _require(
            set(source_entries) == {"windows-native-source-proof.json"},
            "source artifact must contain only windows-native-source-proof.json",
        )
        source_receipt = _source_receipt(
            _json_payload(
                source_entries["windows-native-source-proof.json"],
                "Windows-native source receipt",
            ),
            repository=repository,
            commit=commit,
            tree=tree,
            version=version,
            run=source_run,
        )

        qualification_run = _workflow_run(
            _load_json(qualification_run_json, "qualification run"),
            label="qualification run",
            workflow=QUALIFICATION_WORKFLOW,
            repository=repository,
            commit=commit,
            tree=tree,
        )
        qualification_job = _successful_job(
            _load_json(qualification_jobs_json, "qualification jobs"),
            run=qualification_run,
            commit=commit,
            name=QUALIFICATION_JOB,
            required_steps=QUALIFICATION_STEPS,
        )
        qualification_name = (
            f"windows-x64-vulkan-proof-{version}-attempt-{qualification_run['attempt']}"
        )
        qualification_container, qualification_entries = _artifact(
            _load_json(qualification_artifacts_json, "qualification artifacts"),
            name=qualification_name,
            run=qualification_run,
            commit=commit,
            container=qualification_artifact_zip,
        )
        qualification_payload = _one_basename(
            qualification_entries,
            "qualification.json",
            "qualification artifact container",
        )
        qualification = _qualification(
            _json_payload(qualification_payload, "qualification.json"),
            qualification_entries,
            commit=commit,
            tree=tree,
            version=version,
        )
        return {
            "schema": ACCEPTANCE_SCHEMA,
            "decision": "accept",
            "repository": repository,
            "commit": commit,
            "source_tree": tree,
            "version": version,
            "source_proof": {
                "run": source_run,
                "job": source_job,
                "container": source_container,
                "payload_sha256": _sha256(source_entries["windows-native-source-proof.json"]),
                "contracts": source_receipt["contracts"],
            },
            "qualification": {
                "run": qualification_run,
                "job": qualification_job,
                "container": qualification_container,
                "payload_sha256": _sha256(qualification_payload),
                **qualification,
            },
            "reason": None,
        }
    except AcceptanceRejection as exc:
        return _reject(repository, commit, tree, version, str(exc))


def _write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.tmp-{os.getpid()}")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--tree", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-run-json", required=True, type=Path)
    parser.add_argument("--source-jobs-json", required=True, type=Path)
    parser.add_argument("--source-artifacts-json", required=True, type=Path)
    parser.add_argument("--source-artifact-zip", required=True, type=Path)
    parser.add_argument("--qualification-run-json", required=True, type=Path)
    parser.add_argument("--qualification-jobs-json", required=True, type=Path)
    parser.add_argument("--qualification-artifacts-json", required=True, type=Path)
    parser.add_argument("--qualification-artifact-zip", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    receipt = issue_1634_acceptance(
        repository=args.repository,
        commit=args.commit,
        tree=args.tree,
        version=args.version,
        source_run_json=args.source_run_json,
        source_jobs_json=args.source_jobs_json,
        source_artifacts_json=args.source_artifacts_json,
        source_artifact_zip=args.source_artifact_zip,
        qualification_run_json=args.qualification_run_json,
        qualification_jobs_json=args.qualification_jobs_json,
        qualification_artifacts_json=args.qualification_artifacts_json,
        qualification_artifact_zip=args.qualification_artifact_zip,
    )
    _write_json(args.out, receipt)
    print(json.dumps(receipt, sort_keys=True))
    return 0 if receipt["decision"] == "accept" else 1


if __name__ == "__main__":
    sys.exit(main())
