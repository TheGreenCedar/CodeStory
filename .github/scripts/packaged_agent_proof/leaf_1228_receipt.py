"""Mechanical closure receipt for the Windows native-staging leaf (#1228)."""

from __future__ import annotations

import hashlib
import json
import stat
import zipfile
from pathlib import Path, PurePosixPath


RECEIPT_SCHEMA = "codestory.v017-leaf-artifact-receipt/v1"
RECORD_SCHEMA = "codestory-candidate-archive-store/v1"
SOURCE_PROOF_SCHEMA = "codestory.windows-native-source-proof/v1"
SOURCE_WORKFLOW = ".github/workflows/source-proof.yml"
CANDIDATE_WORKFLOW = ".github/workflows/packaged-platform-pr.yml"
SOURCE_JOB = "windows-native-contracts"
SOURCE_STEP = "Prove Windows path and native-staging source contracts"
CANDIDATE_JOB = "packaged-proof / Build windows-x64"
CANDIDATE_STEPS = (
    "Require exact source identity",
    "Package release asset on Windows",
    "Smoke packaged release asset on Windows",
    "Report fresh package identity",
    "Produce exact candidate archive record",
    "Upload packaged version proof",
    "Run Windows installer ownership self-test",
    "Upload release asset",
    "Upload exact candidate archive record",
)
SHA = frozenset("0123456789abcdef")


class ReceiptRejection(ValueError):
    """The supplied evidence does not prove the leaf claim."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ReceiptRejection(message)


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _digest(value: object, length: int, label: str) -> str:
    _require(
        isinstance(value, str)
        and len(value) == length
        and not (set(value) - SHA),
        f"{label} is not a lowercase hexadecimal digest",
    )
    return value


def _object(value: object, label: str) -> dict:
    _require(isinstance(value, dict), f"{label} must be an object")
    return value


def _list(value: object, label: str) -> list:
    _require(isinstance(value, list), f"{label} must be an array")
    return value


def _load_json(path: Path, label: str) -> dict:
    try:
        return _object(json.loads(path.read_text(encoding="utf-8")), label)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ReceiptRejection(f"{label} is unreadable JSON: {exc}") from exc


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
    _require(document.get("head_sha") == commit, f"{label} commit is not exact F")
    workflow_repository = _object(document.get("repository"), f"{label} repository")
    _require(
        workflow_repository.get("full_name") == repository,
        f"{label} API repository changed",
    )
    head_repository = _object(document.get("head_repository"), f"{label} head repository")
    _require(
        head_repository.get("full_name") == repository,
        f"{label} came from a different repository",
    )
    head_commit = _object(document.get("head_commit"), f"{label} head commit")
    _require(head_commit.get("tree_id") == tree, f"{label} tree is not exact F")
    run_id = document.get("id")
    attempt = document.get("run_attempt")
    _require(isinstance(run_id, int) and run_id > 0, f"{label} run id is invalid")
    _require(isinstance(attempt, int) and attempt > 0, f"{label} attempt is invalid")
    expected_url = f"https://github.com/{repository}/actions/runs/{run_id}"
    _require(document.get("html_url") == expected_url, f"{label} URL changed")
    return {
        "id": run_id,
        "attempt": attempt,
        "url": expected_url,
        "workflow": workflow,
    }


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
    _require(job.get("run_attempt") == run["attempt"], f"{name} belongs to a different attempt")
    _require(job.get("head_sha") == commit, f"{name} commit is not exact F")
    _require(job.get("status") == "completed", f"{name} is not complete")
    _require(job.get("conclusion") == "success", f"{name} did not succeed")
    steps = _list(job.get("steps"), f"{name} steps")
    for required in required_steps:
        matches = [step for step in steps if isinstance(step, dict) and step.get("name") == required]
        _require(len(matches) == 1, f"{name} is missing exact step {required!r}")
        _require(matches[0].get("conclusion") == "success", f"{required} did not succeed")
    job_id = job.get("id")
    _require(isinstance(job_id, int) and job_id > 0, f"{name} job id is invalid")
    return {"id": job_id, "name": name, "steps": list(required_steps)}


def _safe_zip(payload: bytes, label: str) -> dict[str, bytes]:
    entries: dict[str, bytes] = {}
    folded: set[str] = set()
    try:
        with zipfile.ZipFile(__import__("io").BytesIO(payload)) as archive:
            for info in archive.infolist():
                path = PurePosixPath(info.filename)
                _require(
                    info.filename
                    and "\\" not in info.filename
                    and "\0" not in info.filename
                    and not path.is_absolute()
                    and all(part not in ("", ".", "..") for part in path.parts),
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
        raise ReceiptRejection(f"{label} is not a readable ZIP: {exc}") from exc
    _require(bool(entries), f"{label} is empty")
    return entries


def _one_basename(entries: dict[str, bytes], basename: str, label: str) -> tuple[str, bytes]:
    matches = [(name, payload) for name, payload in entries.items() if PurePosixPath(name).name == basename]
    _require(len(matches) == 1, f"{label} must contain exactly one {basename}")
    return matches[0]


def _artifact(
    response: dict,
    *,
    name: str,
    run: dict,
    commit: str,
    container: Path,
) -> tuple[dict, dict[str, bytes]]:
    artifacts = _list(response.get("artifacts"), "artifacts response")
    _require(response.get("total_count") == len(artifacts), "artifacts response is truncated")
    matches = [item for item in artifacts if isinstance(item, dict) and item.get("name") == name]
    _require(len(matches) == 1, f"expected exactly one {name!r} artifact")
    item = matches[0]
    _require(item.get("expired") is False, f"{name} artifact expired")
    workflow_run = _object(item.get("workflow_run"), f"{name} workflow run")
    _require(workflow_run.get("id") == run["id"], f"{name} belongs to a different run")
    _require(workflow_run.get("head_sha") == commit, f"{name} commit is not exact F")
    artifact_id = item.get("id")
    size = item.get("size_in_bytes")
    _require(isinstance(artifact_id, int) and artifact_id > 0, f"{name} id is invalid")
    _require(isinstance(size, int) and size > 0, f"{name} size is invalid")
    try:
        payload = container.read_bytes()
    except OSError as exc:
        raise ReceiptRejection(f"{name} container is unreadable: {exc}") from exc
    _require(len(payload) == size, f"{name} container size differs from GitHub metadata")
    metadata_digest = item.get("digest")
    _require(
        isinstance(metadata_digest, str) and metadata_digest.startswith("sha256:"),
        f"{name} has no GitHub SHA-256 digest",
    )
    digest = _digest(metadata_digest[7:], 64, f"{name} GitHub digest")
    _require(_sha256(payload) == digest, f"{name} container digest differs from GitHub metadata")
    return (
        {"id": artifact_id, "name": name, "bytes": size, "sha256": digest},
        _safe_zip(payload, f"{name} artifact container"),
    )


def _json_entry(entries: dict[str, bytes], basename: str, label: str) -> dict:
    _name, payload = _one_basename(entries, basename, label)
    try:
        return _object(json.loads(payload.decode("utf-8")), label)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ReceiptRejection(f"{label} is not valid JSON: {exc}") from exc


def _record_contract(
    record: dict,
    package_entries: dict[str, bytes],
    *,
    repository: str,
    commit: str,
    tree: str,
    archive_name: str,
) -> bytes:
    _require(
        set(package_entries)
        == {archive_name, f"{archive_name}.sha256", "SHA256SUMS.txt"},
        "package artifact paths differ from the exact release payload",
    )
    _require(
        set(record) == {"schema", "repository", "source", "target", "archive", "companions"},
        "candidate archive record keys changed",
    )
    _require(record.get("schema") == RECORD_SCHEMA, "candidate archive record schema changed")
    _require(record.get("repository") == repository, "candidate archive record repository changed")
    _require(record.get("target") == "windows-x64", "candidate archive record target changed")
    _require(record.get("source") == {"commit": commit, "tree": tree}, "candidate archive record is not exact F")
    archive = _object(record.get("archive"), "candidate archive descriptor")
    _require(
        set(archive) == {"name", "relative_path", "bytes", "sha256"}
        and archive.get("name") == archive_name
        and archive.get("relative_path") == archive_name,
        "candidate archive descriptor does not name the exact Windows archive",
    )
    _archive_path, archive_payload = _one_basename(package_entries, archive_name, "package artifact")
    _require(archive.get("bytes") == len(archive_payload), "candidate archive byte count is stale")
    _require(archive.get("sha256") == _sha256(archive_payload), "candidate archive digest is stale")
    companions = _list(record.get("companions"), "candidate archive companions")
    by_role = {item.get("role"): item for item in companions if isinstance(item, dict)}
    _require(
        len(companions) == 2 and set(by_role) == {"archive_checksum", "checksum_manifest"},
        "candidate archive checksum companions changed",
    )
    expected_names = {
        "archive_checksum": f"{archive_name}.sha256",
        "checksum_manifest": "SHA256SUMS.txt",
    }
    companion_payloads = []
    for role, expected_name in expected_names.items():
        descriptor = by_role[role]
        _require(
            set(descriptor) == {"role", "relative_path", "bytes", "sha256"}
            and descriptor.get("relative_path") == expected_name,
            f"{role} descriptor changed",
        )
        _path, payload = _one_basename(package_entries, expected_name, "package artifact")
        _require(descriptor.get("bytes") == len(payload), f"{role} byte count is stale")
        _require(descriptor.get("sha256") == _sha256(payload), f"{role} digest is stale")
        companion_payloads.append(payload)
    _require(companion_payloads[0] == companion_payloads[1], "checksum companions disagree")
    expected_line = f"{_sha256(archive_payload)}  {archive_name}\n".encode()
    _require(companion_payloads[0] == expected_line, "checksum companions do not bind the archive")
    return archive_payload


def _source_proof_contract(
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
        "Windows-native source proof keys changed",
    )
    _require(document.get("schema") == SOURCE_PROOF_SCHEMA, "Windows-native source proof schema changed")
    _require(document.get("status") == "pass", "Windows-native source proof did not pass")
    _require(document.get("repository") == repository, "Windows-native source proof repository changed")
    _require(document.get("commit") == commit, "Windows-native source proof commit is not exact F")
    _require(document.get("source_tree") == tree, "Windows-native source proof tree is not exact F")
    _require(document.get("version") == version, "Windows-native source proof version changed")
    producer = _object(document.get("producer"), "Windows-native source proof producer")
    _require(
        producer
        == {
            "workflow_path": SOURCE_WORKFLOW,
            "job": SOURCE_JOB,
            "run_id": run["id"],
            "run_attempt": run["attempt"],
        },
        "Windows-native source proof producer identity changed",
    )
    contracts = _object(document.get("contracts"), "Windows-native source contracts")
    _require(
        contracts
        == {
            "qualification_harness_self_test": True,
            "control_directory_inflight": True,
            "windows_path_identity": True,
            "native_staging": True,
        },
        "Windows-native source contracts are incomplete",
    )
    return document


def _native_contract(
    archive_payload: bytes,
    version_entries: dict[str, bytes],
    *,
    version: str,
    commit: str,
    tree: str,
) -> dict:
    _require(
        set(version_entries) == {"summary.json"},
        "packaged version proof artifact paths changed",
    )
    archive_entries = _safe_zip(archive_payload, "Windows candidate archive")
    _manifest_name, manifest_payload = _one_basename(
        archive_entries, "codestory-native-manifest.json", "Windows candidate archive"
    )
    try:
        manifest = _object(json.loads(manifest_payload.decode("utf-8")), "native manifest")
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ReceiptRejection(f"native manifest is not valid JSON: {exc}") from exc
    version_summary = _json_entry(version_entries, "summary.json", "packaged version proof summary")
    package_contract = _object(version_summary.get("package_contract"), "version proof package contract")
    _require(package_contract.get("manifest") == manifest, "version proof manifest differs from final archive")
    _require(manifest.get("schema_version") == 3, "native manifest schema changed")
    _require(manifest.get("release_version") == version, "native manifest release version changed")
    _require(manifest.get("asset_target") == "windows-x64", "native manifest target changed")
    _require(
        manifest.get("source") == {"commit": commit, "tree": tree, "tracked_dirty": False},
        "native manifest source is not clean exact F",
    )
    binary = _object(manifest.get("binary"), "native manifest binary")
    _require(binary.get("name") == "codestory-cli.exe", "native manifest launcher name changed")
    _launcher_name, launcher = _one_basename(archive_entries, "codestory-cli.exe", "Windows candidate archive")
    launcher_digest = _digest(binary.get("sha256"), 64, "launcher digest")
    _require(_sha256(launcher) == launcher_digest, "final launcher bytes differ from native manifest")
    runtime = _object(manifest.get("runtime_executable"), "native manifest runtime executable")
    generation = _digest(runtime.get("generation_id"), 64, "native runtime generation")
    runtime_name = runtime.get("name")
    _require(
        isinstance(runtime_name, str) and PurePosixPath(runtime_name).name == runtime_name,
        "native runtime name is not a basename",
    )
    pointer_name, pointer = _one_basename(
        archive_entries,
        "codestory-native-current-generation-v1.txt",
        "Windows candidate archive",
    )
    _require(pointer.decode("utf-8").strip() == generation, "native generation pointer differs from manifest")
    _require("/" in pointer_name, "native generation pointer is outside the package root")
    root = pointer_name.rsplit("/", 1)[0]
    runtime_path = f"{root}/codestory-native-generations/{generation}/{runtime_name}"
    _require(runtime_path in archive_entries, "final generation runtime is missing")
    runtime_digest = _digest(runtime.get("sha256"), 64, "native runtime digest")
    _require(_sha256(archive_entries[runtime_path]) == runtime_digest, "final runtime bytes differ from native manifest")
    descriptors = _list(manifest.get("runtime_artifacts"), "native runtime artifacts")
    artifact_receipts = []
    for descriptor in descriptors:
        descriptor = _object(descriptor, "native runtime artifact descriptor")
        name = descriptor.get("name")
        _require(isinstance(name, str) and PurePosixPath(name).name == name, "runtime artifact name is not a basename")
        path = f"{root}/codestory-native-generations/{generation}/{name}"
        _require(path in archive_entries, f"final runtime artifact is missing: {name}")
        digest = _digest(descriptor.get("sha256"), 64, f"runtime artifact {name} digest")
        _require(_sha256(archive_entries[path]) == digest, f"runtime artifact bytes differ: {name}")
        artifact_receipts.append({"name": name, "sha256": digest})
    _require(bool(artifact_receipts), "Windows native manifest has no runtime module artifacts")
    _require(len({item["name"].casefold() for item in artifact_receipts}) == len(artifact_receipts), "runtime artifact names collide on Windows")
    _require(runtime_path.casefold() != _launcher_name.casefold(), "launcher and managed runtime are not distinct archive entries")
    return {
        "manifest_sha256": _sha256(manifest_payload),
        "launcher_sha256": launcher_digest,
        "runtime_generation": generation,
        "runtime_sha256": runtime_digest,
        "runtime_artifacts": sorted(artifact_receipts, key=lambda item: item["name"].casefold()),
        "final_entries_are_distinct_regular_files": True,
    }


def _receipt(payload: dict) -> dict:
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return {**payload, "receipt_sha256": _sha256(encoded)}


def build_issue_1228_receipt(
    *,
    repository: str,
    candidate_sha: str,
    candidate_tree: str,
    version: str,
    source_run_json: Path,
    source_jobs_json: Path,
    source_artifacts_json: Path,
    source_proof_artifact_container: Path,
    candidate_run_json: Path,
    candidate_jobs_json: Path,
    candidate_artifacts_json: Path,
    package_artifact_container: Path,
    archive_record_artifact_container: Path,
    version_proof_artifact_container: Path,
) -> dict:
    """Validate exact-F source/package/native evidence and return one receipt."""
    candidate = {"repository": repository, "commit": candidate_sha, "tree": candidate_tree, "version": version}
    try:
        candidate_sha = _digest(candidate_sha, 40, "candidate SHA")
        candidate_tree = _digest(candidate_tree, 40, "candidate tree")
        _require(repository.count("/") == 1, "repository must use owner/name")
        _require(bool(version), "version must be non-empty")
        source_run = _workflow_run(
            _load_json(source_run_json, "source run"),
            label="source run",
            workflow=SOURCE_WORKFLOW,
            repository=repository,
            commit=candidate_sha,
            tree=candidate_tree,
        )
        source_job = _successful_job(
            _load_json(source_jobs_json, "source jobs"),
            run=source_run,
            commit=candidate_sha,
            name=SOURCE_JOB,
            required_steps=(SOURCE_STEP,),
        )
        source_artifact_name = (
            f"windows-native-source-proof-{candidate_sha}-attempt-{source_run['attempt']}"
        )
        source_artifact_id, source_entries = _artifact(
            _load_json(source_artifacts_json, "source artifacts"),
            name=source_artifact_name,
            run=source_run,
            commit=candidate_sha,
            container=source_proof_artifact_container,
        )
        _require(
            set(source_entries) == {"windows-native-source-proof.json"},
            "Windows-native source proof artifact paths changed",
        )
        source_proof = _source_proof_contract(
            _json_entry(source_entries, "windows-native-source-proof.json", "Windows-native source proof"),
            repository=repository,
            commit=candidate_sha,
            tree=candidate_tree,
            version=version,
            run=source_run,
        )
        candidate_run = _workflow_run(
            _load_json(candidate_run_json, "candidate run"),
            label="candidate run",
            workflow=CANDIDATE_WORKFLOW,
            repository=repository,
            commit=candidate_sha,
            tree=candidate_tree,
        )
        candidate_job = _successful_job(
            _load_json(candidate_jobs_json, "candidate jobs"),
            run=candidate_run,
            commit=candidate_sha,
            name=CANDIDATE_JOB,
            required_steps=CANDIDATE_STEPS,
        )
        artifacts = _load_json(candidate_artifacts_json, "candidate artifacts")
        package_name = "codestory-cli-windows-x64"
        record_name = "codestory-candidate-archive-record-windows-x64"
        version_name = f"packaged-version-proof-windows-x64-attempt-{candidate_run['attempt']}"
        package_id, package_entries = _artifact(
            artifacts, name=package_name, run=candidate_run, commit=candidate_sha, container=package_artifact_container
        )
        record_id, record_entries = _artifact(
            artifacts, name=record_name, run=candidate_run, commit=candidate_sha, container=archive_record_artifact_container
        )
        version_id, version_entries = _artifact(
            artifacts, name=version_name, run=candidate_run, commit=candidate_sha, container=version_proof_artifact_container
        )
        archive_name = f"codestory-cli-v{version}-windows-x64.zip"
        record = _json_entry(record_entries, "candidate-archive-record.json", "candidate archive record")
        _require(
            set(record_entries) == {"candidate-archive-record.json"},
            "candidate archive record artifact paths changed",
        )
        archive_payload = _record_contract(
            record,
            package_entries,
            repository=repository,
            commit=candidate_sha,
            tree=candidate_tree,
            archive_name=archive_name,
        )
        native = _native_contract(
            archive_payload,
            version_entries,
            version=version,
            commit=candidate_sha,
            tree=candidate_tree,
        )
        payload = {
            "schema": RECEIPT_SCHEMA,
            "issue": 1228,
            "claim": "exact_f_windows_native_staging_and_final_package_binding",
            "decision": "accept",
            "candidate": candidate,
            "evidence": {
                "source_run": source_run,
                "source_job": source_job,
                "source_artifact": source_artifact_id,
                "source_contracts": source_proof["contracts"],
                "candidate_run": candidate_run,
                "candidate_job": candidate_job,
                "artifacts": [package_id, record_id, version_id],
                "archive_sha256": _sha256(archive_payload),
                "native": native,
                "independent_copy_source_contract": True,
            },
            "rejection": None,
        }
    except ReceiptRejection as exc:
        payload = {
            "schema": RECEIPT_SCHEMA,
            "issue": 1228,
            "claim": "exact_f_windows_native_staging_and_final_package_binding",
            "decision": "reject",
            "candidate": candidate,
            "evidence": {},
            "rejection": {"code": "contract_rejected", "message": str(exc)},
        }
    return _receipt(payload)


def write_receipt(path: Path, receipt: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
