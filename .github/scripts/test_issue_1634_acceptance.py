#!/usr/bin/env python3

from __future__ import annotations

import copy
import hashlib
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path


SCRIPT = Path(__file__).with_name("issue_1634_acceptance.py")
SPEC = importlib.util.spec_from_file_location("issue_1634_acceptance", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

REPOSITORY = "TheGreenCedar/CodeStory"
COMMIT = "a" * 40
TREE = "b" * 40
VERSION = "0.17.0"


def _json(value: dict) -> bytes:
    return (json.dumps(value, sort_keys=True) + "\n").encode()


def _zip(entries: dict[str, bytes]) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, payload in entries.items():
            archive.writestr(name, payload)
    return output.getvalue()


def _run(run_id: int, attempt: int, workflow: str) -> dict:
    return {
        "id": run_id,
        "run_attempt": attempt,
        "path": workflow,
        "event": "workflow_dispatch",
        "status": "completed",
        "conclusion": "success",
        "head_sha": COMMIT,
        "repository": {"full_name": REPOSITORY},
        "head_repository": {"full_name": REPOSITORY},
        "head_commit": {"tree_id": TREE},
        "html_url": f"https://github.com/{REPOSITORY}/actions/runs/{run_id}",
    }


def _jobs(run: dict, job_id: int, name: str, steps: tuple[str, ...]) -> dict:
    return {
        "total_count": 1,
        "jobs": [
            {
                "id": job_id,
                "name": name,
                "run_id": run["id"],
                "run_attempt": run["run_attempt"],
                "head_sha": COMMIT,
                "status": "completed",
                "conclusion": "success",
                "steps": [{"name": step, "conclusion": "success"} for step in steps],
            }
        ],
    }


def _artifacts(run: dict, artifact_id: int, name: str, payload: bytes) -> dict:
    return {
        "total_count": 1,
        "artifacts": [
            {
                "id": artifact_id,
                "name": name,
                "size_in_bytes": len(payload),
                "digest": f"sha256:{hashlib.sha256(payload).hexdigest()}",
                "expired": False,
                "workflow_run": {"id": run["id"], "head_sha": COMMIT},
            }
        ],
    }


def _documents() -> dict:
    source_run = _run(41001, 2, MODULE.SOURCE_WORKFLOW)
    qualification_run = _run(42001, 3, MODULE.QUALIFICATION_WORKFLOW)
    source_receipt = {
        "schema": MODULE.SOURCE_RECEIPT_SCHEMA,
        "status": "pass",
        "repository": REPOSITORY,
        "commit": COMMIT,
        "source_tree": TREE,
        "version": VERSION,
        "producer": {
            "workflow_path": MODULE.SOURCE_WORKFLOW,
            "job": MODULE.SOURCE_JOB,
            "run_id": source_run["id"],
            "run_attempt": source_run["run_attempt"],
        },
        "contracts": {
            "qualification_harness_self_test": True,
            "control_directory_inflight": True,
            "windows_path_identity": True,
            "native_staging": True,
        },
    }
    retained = {
        "server_crash.json": b"server crash evidence\n",
        "publication-fault-external.raw.json": b"publication evidence\n",
    }
    qualification = {
        "schema_version": 1,
        "status": "pass",
        "source": {"commit": COMMIT, "tree": TREE},
        "package": {"release_version": VERSION},
        "scenarios": {
            "server_crash": {
                "status": "pass",
                "assertions": {
                    "one_replacement_server": True,
                    "pure_embedding_rpc_replayed_at_most_once": True,
                },
                "artifacts": [
                    {"name": name, "sha256": hashlib.sha256(payload).hexdigest()}
                    for name, payload in retained.items()
                ],
            }
        },
    }
    return {
        "source_run": source_run,
        "source_jobs": _jobs(source_run, 51001, MODULE.SOURCE_JOB, MODULE.SOURCE_STEPS),
        "source_receipt": source_receipt,
        "qualification_run": qualification_run,
        "qualification_jobs": _jobs(
            qualification_run,
            52001,
            MODULE.QUALIFICATION_JOB,
            MODULE.QUALIFICATION_STEPS,
        ),
        "qualification": qualification,
        "retained": retained,
    }


def _materialize(root: Path, documents: dict) -> dict[str, Path]:
    source_zip = _zip({"windows-native-source-proof.json": _json(documents["source_receipt"])})
    qualification_zip = _zip(
        {
            "proof/qualification.json": _json(documents["qualification"]),
            **{f"proof/raw/{name}": payload for name, payload in documents["retained"].items()},
        }
    )
    source_name = (
        f"windows-native-source-proof-{COMMIT}-attempt-"
        f"{documents['source_run']['run_attempt']}"
    )
    qualification_name = (
        f"windows-x64-vulkan-proof-{VERSION}-attempt-"
        f"{documents['qualification_run']['run_attempt']}"
    )
    documents["source_artifacts"] = _artifacts(
        documents["source_run"], 61001, source_name, source_zip
    )
    documents["qualification_artifacts"] = _artifacts(
        documents["qualification_run"], 62001, qualification_name, qualification_zip
    )
    payloads = {
        "source_run_json": _json(documents["source_run"]),
        "source_jobs_json": _json(documents["source_jobs"]),
        "source_artifacts_json": _json(documents["source_artifacts"]),
        "source_artifact_zip": source_zip,
        "qualification_run_json": _json(documents["qualification_run"]),
        "qualification_jobs_json": _json(documents["qualification_jobs"]),
        "qualification_artifacts_json": _json(documents["qualification_artifacts"]),
        "qualification_artifact_zip": qualification_zip,
    }
    paths = {}
    for name, payload in payloads.items():
        path = root / name
        path.write_bytes(payload)
        paths[name] = path
    return paths


def _evaluate(mutate=lambda _value: None, after=lambda _value, _paths: None) -> dict:
    documents = _documents()
    mutate(documents)
    with tempfile.TemporaryDirectory() as directory:
        paths = _materialize(Path(directory), documents)
        after(documents, paths)
        return MODULE.issue_1634_acceptance(
            repository=REPOSITORY,
            commit=COMMIT,
            tree=TREE,
            version=VERSION,
            **paths,
        )


class Issue1634AcceptanceTest(unittest.TestCase):
    def assert_reject(self, receipt: dict, message: str) -> None:
        self.assertEqual(receipt["schema"], MODULE.ACCEPTANCE_SCHEMA)
        self.assertEqual(receipt["decision"], "reject")
        self.assertIn(message, receipt["reason"])

    def test_accepts_exact_source_and_qualification_artifacts(self) -> None:
        receipt = _evaluate()
        self.assertEqual(receipt["decision"], "accept")
        self.assertEqual(receipt["source_proof"]["run"]["id"], 41001)
        self.assertEqual(receipt["qualification"]["run"]["id"], 42001)
        self.assertEqual(len(receipt["qualification"]["retained_artifacts"]), 2)

    def test_rejects_wrong_head(self) -> None:
        def mutate(value: dict) -> None:
            value["source_run"]["head_sha"] = "f" * 40

        self.assert_reject(_evaluate(mutate), "commit is not exact")

    def test_rejects_wrong_tree(self) -> None:
        def mutate(value: dict) -> None:
            value["qualification_run"]["head_commit"]["tree_id"] = "f" * 40

        self.assert_reject(_evaluate(mutate), "tree is not exact")

    def test_rejects_wrong_run(self) -> None:
        def after(value: dict, paths: dict[str, Path]) -> None:
            artifact = copy.deepcopy(value["source_artifacts"])
            artifact["artifacts"][0]["workflow_run"]["id"] = 99999
            paths["source_artifacts_json"].write_bytes(_json(artifact))

        self.assert_reject(_evaluate(after=after), "different run")

    def test_rejects_wrong_attempt(self) -> None:
        def mutate(value: dict) -> None:
            value["qualification_jobs"]["jobs"][0]["run_attempt"] = 9

        self.assert_reject(_evaluate(mutate), "different attempt")

    def test_rejects_missing_source_marker(self) -> None:
        def mutate(value: dict) -> None:
            del value["source_receipt"]["contracts"]["control_directory_inflight"]

        self.assert_reject(_evaluate(mutate), "control-directory/inflight")

    def test_rejects_failed_server_crash(self) -> None:
        def mutate(value: dict) -> None:
            value["qualification"]["scenarios"]["server_crash"]["status"] = "fail"

        self.assert_reject(_evaluate(mutate), "server_crash status is not pass")

    def test_rejects_false_server_crash_assertion(self) -> None:
        def mutate(value: dict) -> None:
            value["qualification"]["scenarios"]["server_crash"]["assertions"][
                "one_replacement_server"
            ] = False

        self.assert_reject(_evaluate(mutate), "every server_crash assertion")

    def test_rejects_missing_retained_references(self) -> None:
        def mutate(value: dict) -> None:
            value["qualification"]["scenarios"]["server_crash"]["artifacts"] = []

        self.assert_reject(_evaluate(mutate), "retained artifact references are empty")

    def test_rejects_wrong_artifact_identity(self) -> None:
        def after(value: dict, paths: dict[str, Path]) -> None:
            artifacts = copy.deepcopy(value["qualification_artifacts"])
            artifacts["artifacts"][0]["name"] = "wrong-artifact"
            paths["qualification_artifacts_json"].write_bytes(_json(artifacts))

        self.assert_reject(_evaluate(after=after), "expected exactly one")

    def test_rejects_forged_artifact_digest(self) -> None:
        def after(value: dict, paths: dict[str, Path]) -> None:
            artifacts = copy.deepcopy(value["qualification_artifacts"])
            artifacts["artifacts"][0]["digest"] = f"sha256:{'f' * 64}"
            paths["qualification_artifacts_json"].write_bytes(_json(artifacts))

        self.assert_reject(_evaluate(after=after), "container digest differs")

    def test_rejects_substituted_artifact_container(self) -> None:
        def after(_value: dict, paths: dict[str, Path]) -> None:
            paths["qualification_artifact_zip"].write_bytes(_zip({"forged.json": b"{}\n"}))

        self.assert_reject(_evaluate(after=after), "container size differs")

    def test_cli_writes_machine_reject_receipt(self) -> None:
        documents = _documents()
        documents["qualification"]["scenarios"]["server_crash"]["status"] = "fail"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = _materialize(root, documents)
            output = root / "receipt.json"
            command = [
                sys.executable,
                str(SCRIPT),
                "--repository",
                REPOSITORY,
                "--commit",
                COMMIT,
                "--tree",
                TREE,
                "--version",
                VERSION,
            ]
            for flag, name in (
                ("--source-run-json", "source_run_json"),
                ("--source-jobs-json", "source_jobs_json"),
                ("--source-artifacts-json", "source_artifacts_json"),
                ("--source-artifact-zip", "source_artifact_zip"),
                ("--qualification-run-json", "qualification_run_json"),
                ("--qualification-jobs-json", "qualification_jobs_json"),
                ("--qualification-artifacts-json", "qualification_artifacts_json"),
                ("--qualification-artifact-zip", "qualification_artifact_zip"),
            ):
                command.extend((flag, str(paths[name])))
            command.extend(("--out", str(output)))
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 1, result.stderr)
            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(receipt["schema"], MODULE.ACCEPTANCE_SCHEMA)
            self.assertEqual(receipt["decision"], "reject")


if __name__ == "__main__":
    unittest.main()
