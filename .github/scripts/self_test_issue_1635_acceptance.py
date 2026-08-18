#!/usr/bin/env python3
"""Focused accept and hostile-mutation tests for issue_1635_acceptance.py."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

from issue_1635_acceptance import RECEIPT_SCHEMA, assess_issue_1635


REPOSITORY = "TheGreenCedar/CodeStory"
COMMIT = "a" * 40
TREE = "b" * 40
OTHER_SHA = "c" * 40
VERSION = "0.17.0"
RUN_ID = 1635001
ATTEMPT = 1
JOB_NAME = "packaged-proof / Build windows-x64"
STARTED = "2026-08-09T12:00:00Z"
CREATED = "2026-08-09T12:05:00Z"
COMPLETED = "2026-08-09T12:10:00Z"
SCRIPT = Path(__file__).with_name("issue_1635_acceptance.py")


def _sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _json(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


class Fixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.driver = b"exact qualification driver bytes"
        driver_digest = _sha256(self.driver)
        self.build = {
            "schema": "codestory.cargo-build-artifacts/v2",
            "source": {"commit": COMMIT, "tree": TREE},
            "build": {
                "rust_target": "x86_64-pc-windows-msvc",
                "profile": "release",
                "target_dir": "D:\\t",
                "workspace_root": "D:\\a\\CodeStory\\CodeStory",
            },
            "artifacts": {
                "cli": {},
                "runtime": {},
                "qualification_driver": {
                    "package": "codestory-bench",
                    "target": "codestory_embedding_qualification",
                    "kind": "bin",
                    "profile": {
                        "opt_level": "3",
                        "debuginfo": 0,
                        "debug_assertions": False,
                        "overflow_checks": False,
                        "test": False,
                    },
                    "path": "D:\\t\\codestory_embedding_qualification.exe",
                    "relative_path": "codestory_embedding_qualification.exe",
                    "bytes": len(self.driver),
                    "sha256": driver_digest,
                    "native_links": {},
                },
            },
        }
        self.qualification = {
            "schema_version": 1,
            "source": {"commit": COMMIT, "tree": TREE},
            "release_version": VERSION,
            "asset_target": "windows-x64",
            "archive": {
                "file": f"codestory-cli-v{VERSION}-windows-x64.zip",
                "bytes": 8192,
                "sha256": "d" * 64,
            },
            "driver": {
                "file": "codestory_embedding_qualification.exe",
                "bytes": len(self.driver),
                "sha256": driver_digest,
            },
        }
        self.timing = {
            "schema": "codestory.windows-link-timing/v1",
            "phase": "msvc_link",
            "source": "msvc-link-time-report",
            "status": "observed",
            "reason": None,
            "observational": True,
            "invocation_count": 1,
            "link_ms": 1250,
            "invocations": [{
                "index": 1,
                "intervals": [
                    {"pass": 1, "interval": 1, "seconds": 0.5},
                    {"pass": 2, "interval": 2, "seconds": 0.5},
                ],
                "total_seconds": 1.25,
                "total_ms": 1250,
            }],
            "build_interval": {"phase": "cargo_graph", "elapsed_ms": 8000},
            "diagnostics": {
                "truncated_reports": 0,
                "orphan_totals": 0,
                "dangling_intervals": 0,
            },
        }
        self.run = {
            "id": RUN_ID,
            "run_attempt": ATTEMPT,
            "head_sha": COMMIT,
            "head_commit": {"tree_id": TREE},
            "head_repository": {"full_name": REPOSITORY},
            "repository": {"full_name": REPOSITORY},
            "path": ".github/workflows/packaged-platform-pr.yml",
            "event": "workflow_dispatch",
            "status": "in_progress",
            "conclusion": None,
            "html_url": f"https://github.com/{REPOSITORY}/actions/runs/{RUN_ID}",
        }
        self.jobs = {
            "total_count": 1,
            "jobs": [{
                "id": 7001,
                "run_id": RUN_ID,
                "run_attempt": ATTEMPT,
                "name": JOB_NAME,
                "head_sha": COMMIT,
                "status": "completed",
                "conclusion": "success",
                "started_at": STARTED,
                "completed_at": COMPLETED,
                "steps": [
                    {"name": name, "conclusion": "success"}
                    for name in (
                        "Require exact source identity",
                        "Build package and qualification driver",
                        "Stage qualification driver in package proof artifact",
                        "Upload separate qualification driver",
                        "Upload Windows package build timing",
                    )
                ],
            }]
        }
        self.qualification_container = root / "qualification.zip"
        self.timing_container = root / "timing.zip"
        self.run_path = root / "run.json"
        self.jobs_path = root / "jobs.json"
        self.artifacts_path = root / "artifacts.json"
        self.receipt_path = root / "receipt.json"
        self.artifacts = {
            "total_count": 2,
            "artifacts": [
                self._artifact(8101, "codestory-qualification-driver-windows-x64"),
                self._artifact(8102, "windows-package-build-timing-attempt-1"),
            ]
        }
        self.include_timing = True
        self.include_link_log = True
        self.link_log = (
            b"Pass 1: Interval #1, time = 0.50000s\n"
            b"Pass 2: Interval #2, time = 0.50000s\n"
            b"Final: Total time = 1.25000s\n"
        )
        self.refresh_qualification_container()
        self.refresh_timing_container()

    @staticmethod
    def _artifact(identifier: int, name: str) -> dict[str, object]:
        return {
            "id": identifier,
            "name": name,
            "size_in_bytes": 1,
            "digest": f"sha256:{'0' * 64}",
            "expired": False,
            "created_at": CREATED,
            "expires_at": "2026-09-08T12:05:00Z",
            "workflow_run": {"id": RUN_ID, "head_sha": COMMIT},
        }

    def _write_zip(self, path: Path, members: dict[str, bytes]) -> None:
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as archive:
            for name, value in members.items():
                archive.writestr(name, value)

    def _bind_container(self, index: int, path: Path) -> None:
        value = path.read_bytes()
        self.artifacts["artifacts"][index]["size_in_bytes"] = len(value)
        self.artifacts["artifacts"][index]["digest"] = f"sha256:{_sha256(value)}"

    def refresh_qualification_container(self) -> None:
        self._write_zip(self.qualification_container, {
            "qualification-driver-identity.json": _json(self.qualification),
            "codestory_embedding_qualification.exe": self.driver,
        })
        self._bind_container(0, self.qualification_container)

    def refresh_timing_container(self) -> None:
        members = {
            "cargo-build-artifacts.json": _json(self.build),
        }
        if self.include_link_log:
            members["msvc-link-time.log"] = self.link_log
        if self.include_timing:
            members["windows-link-timing.json"] = _json(self.timing)
        self._write_zip(self.timing_container, members)
        self._bind_container(1, self.timing_container)

    def write_api_evidence(self) -> None:
        self.run_path.write_bytes(_json(self.run))
        self.jobs_path.write_bytes(_json(self.jobs))
        self.artifacts_path.write_bytes(_json(self.artifacts))

    def assess(self) -> dict[str, object]:
        self.write_api_evidence()
        return assess_issue_1635(
            repository=REPOSITORY,
            commit=COMMIT,
            tree=TREE,
            version=VERSION,
            run_id=RUN_ID,
            run_attempt=ATTEMPT,
            run_evidence=str(self.run_path),
            job_evidence=str(self.jobs_path),
            artifact_evidence=str(self.artifacts_path),
            qualification_container=str(self.qualification_container),
            timing_container=str(self.timing_container),
        )

    def cli(self) -> subprocess.CompletedProcess[str]:
        self.write_api_evidence()
        return subprocess.run([
            sys.executable,
            str(SCRIPT),
            "verify",
            "--repository", REPOSITORY,
            "--commit", COMMIT,
            "--tree", TREE,
            "--version", VERSION,
            "--run-id", str(RUN_ID),
            "--run-attempt", str(ATTEMPT),
            "--run-evidence", str(self.run_path),
            "--job-evidence", str(self.jobs_path),
            "--artifact-evidence", str(self.artifacts_path),
            "--qualification-container", str(self.qualification_container),
            "--timing-container", str(self.timing_container),
            "--out", str(self.receipt_path),
        ], check=False, capture_output=True, text=True)


class Issue1635AcceptanceTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="codestory-1635-")
        self.addCleanup(temporary.cleanup)
        self.fixture = Fixture(Path(temporary.name))

    def assert_reject(self, fragment: str) -> None:
        receipt = self.fixture.assess()
        self.assertEqual(receipt["schema"], RECEIPT_SCHEMA)
        self.assertEqual(receipt["decision"], "reject")
        self.assertIn(fragment, str(receipt["reason"]))

    def test_accepts_exact_observed_link_timing(self) -> None:
        receipt = self.fixture.assess()
        self.assertEqual(receipt["schema"], RECEIPT_SCHEMA)
        self.assertEqual(receipt["decision"], "accept")
        self.assertIsNone(receipt["reason"])
        self.assertEqual(receipt["producer"]["run_id"], RUN_ID)
        self.assertEqual(receipt["archive"]["sha256"], "d" * 64)
        self.assertEqual(receipt["evidence"]["timing"]["link_ms"], 1250)

    def test_rejects_unavailable_timing(self) -> None:
        self.fixture.timing.update(status="unavailable", reason="no-explicit-linker-report")
        self.fixture.refresh_timing_container()
        self.assert_reject("status is not observed")

    def test_rejects_missing_timing(self) -> None:
        self.fixture.include_timing = False
        self.fixture.refresh_timing_container()
        self.assert_reject("Windows link timing is missing")

    def test_rejects_missing_retained_linker_log(self) -> None:
        self.fixture.include_link_log = False
        self.fixture.refresh_timing_container()
        self.assert_reject("retained linker log is missing")

    def test_rejects_compile_timing_line_as_linker_evidence(self) -> None:
        self.fixture.link_log = b"   Compiling time v0.3.47\n"
        self.fixture.refresh_timing_container()
        self.assert_reject("no complete explicit linker invocation")

    def test_rejects_timing_json_that_disagrees_with_retained_log(self) -> None:
        self.fixture.timing["link_ms"] = 1251
        self.fixture.refresh_timing_container()
        self.assert_reject("does not match the retained linker log")

    def test_rejects_zero_invocations(self) -> None:
        self.fixture.timing.update(invocation_count=0, invocations=[])
        self.fixture.refresh_timing_container()
        self.assert_reject("invocation_count must be a positive integer")

    def test_rejects_nonnumeric_timing(self) -> None:
        self.fixture.timing["link_ms"] = "1250"
        self.fixture.refresh_timing_container()
        self.assert_reject("finite non-negative number")

    def test_rejects_negative_timing(self) -> None:
        self.fixture.timing["link_ms"] = -1
        self.fixture.refresh_timing_container()
        self.assert_reject("finite non-negative number")

    def test_rejects_wrong_head(self) -> None:
        self.fixture.run["head_sha"] = OTHER_SHA
        self.assert_reject("Actions run identity")

    def test_rejects_wrong_tree(self) -> None:
        self.fixture.build["source"]["tree"] = OTHER_SHA
        self.fixture.refresh_timing_container()
        self.assert_reject("exact head and tree")

    def test_rejects_wrong_run(self) -> None:
        self.fixture.artifacts["artifacts"][1]["workflow_run"]["id"] = RUN_ID + 1
        self.assert_reject("stale run or head identity")

    def test_rejects_wrong_attempt(self) -> None:
        self.fixture.run["run_attempt"] = ATTEMPT + 1
        self.assert_reject("Actions run identity")

    def test_rejects_wrong_job(self) -> None:
        self.fixture.jobs["jobs"][0]["name"] = "another job"
        self.assert_reject("exactly one expected Windows build job")

    def test_rejects_wrong_artifact(self) -> None:
        self.fixture.artifacts["artifacts"][1]["name"] = "forged-link-timing"
        self.assert_reject("must retain exactly one windows-package-build-timing")

    def test_rejects_wrong_container(self) -> None:
        with self.fixture.timing_container.open("ab") as output:
            output.write(b"forged")
        self.assert_reject("container bytes do not match")

    def test_rejects_wrong_schema(self) -> None:
        self.fixture.timing["schema"] = "codestory.windows-link-timing/v2"
        self.fixture.refresh_timing_container()
        self.assert_reject("schema is not")

    def test_rejects_wrong_phase(self) -> None:
        self.fixture.timing["phase"] = "cargo_graph"
        self.fixture.refresh_timing_container()
        self.assert_reject("phase is not msvc_link")

    def test_rejects_wrong_repository(self) -> None:
        self.fixture.run["repository"]["full_name"] = "Elsewhere/CodeStory"
        self.assert_reject("Actions run identity")

    def test_rejects_wrong_version(self) -> None:
        self.fixture.qualification["release_version"] = "0.16.3"
        self.fixture.refresh_qualification_container()
        self.assert_reject("exact Windows candidate")

    def test_rejects_wrong_archive_identity(self) -> None:
        self.fixture.qualification["archive"]["file"] = "other.zip"
        self.fixture.refresh_qualification_container()
        self.assert_reject("exact Windows candidate")

    def test_rejects_forged_identity_and_timing_payload(self) -> None:
        self.fixture.build["artifacts"]["qualification_driver"]["sha256"] = "e" * 64
        self.fixture.qualification["driver"]["sha256"] = "e" * 64
        self.fixture.timing["link_ms"] = 1
        self.fixture.refresh_qualification_container()
        self.fixture.refresh_timing_container()
        self.assert_reject("does not match the exact Windows build identity")

    def test_cli_writes_accept_and_reject_receipts(self) -> None:
        accepted = self.fixture.cli()
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        self.assertEqual(json.loads(self.fixture.receipt_path.read_text())["decision"], "accept")
        self.fixture.timing["status"] = "unavailable"
        self.fixture.timing["reason"] = "no-explicit-linker-report"
        self.fixture.refresh_timing_container()
        rejected = self.fixture.cli()
        self.assertEqual(rejected.returncode, 1, rejected.stderr)
        self.assertEqual(json.loads(self.fixture.receipt_path.read_text())["decision"], "reject")


if __name__ == "__main__":
    unittest.main(verbosity=2)
