#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

SCRIPT = Path(__file__).with_name("extract-candidate-actions-artifact.py")
SPEC = importlib.util.spec_from_file_location("candidate_extract", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


class CandidateArtifactExtractionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.archive_name = "codestory-cli-v0.16.3-windows-x64.zip"
        self.payloads = {
            self.archive_name: b"candidate bytes",
            f"{self.archive_name}.sha256": b"archive checksum\n",
            "SHA256SUMS.txt": b"archive checksum\n",
        }
        self.record = {
            "schema": "codestory-candidate-archive-store/v1",
            "repository": "TheGreenCedar/CodeStory",
            "source": {"commit": "a" * 40, "tree": "b" * 40},
            "target": "windows-x64",
            "archive": {
                "name": self.archive_name,
                "relative_path": self.archive_name,
                "bytes": len(self.payloads[self.archive_name]),
                "sha256": sha256(self.payloads[self.archive_name]),
            },
            "companions": [
                {
                    "role": "archive_checksum",
                    "relative_path": f"{self.archive_name}.sha256",
                    "bytes": len(self.payloads[f"{self.archive_name}.sha256"]),
                    "sha256": sha256(
                        self.payloads[f"{self.archive_name}.sha256"]
                    ),
                },
                {
                    "role": "checksum_manifest",
                    "relative_path": "SHA256SUMS.txt",
                    "bytes": len(self.payloads["SHA256SUMS.txt"]),
                    "sha256": sha256(self.payloads["SHA256SUMS.txt"]),
                },
            ],
        }
        self.record_path = self.root / "candidate-archive-record.json"
        self.write_record()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_record(self) -> None:
        self.record_path.write_text(json.dumps(self.record), encoding="utf-8")

    def write_artifact(
        self,
        payloads: dict[str, bytes] | None = None,
        *,
        duplicate: str | None = None,
    ) -> Path:
        path = self.root / "artifact.zip"
        with zipfile.ZipFile(path, "w") as bundle:
            for name, value in (payloads or self.payloads).items():
                bundle.writestr(name, value)
            if duplicate is not None:
                bundle.writestr(duplicate, b"duplicate")
        return path

    def test_extracts_only_the_exact_public_candidate_payload(self) -> None:
        artifact = self.write_artifact()
        output = self.root / "staged"
        MODULE.extract(artifact, self.record_path, output)
        self.assertEqual(
            {path.name: path.read_bytes() for path in output.iterdir()},
            self.payloads,
        )

    def test_rejects_qualification_material_in_the_public_record(self) -> None:
        self.record["companions"].append(
            {
                "role": "qualification_driver",
                "relative_path": "qualification.exe",
                "bytes": 1,
                "sha256": "c" * 64,
            }
        )
        self.write_record()
        with self.assertRaisesRegex(ValueError, "two public checksum"):
            MODULE.extract(
                self.write_artifact(),
                self.record_path,
                self.root / "staged",
            )

    def test_rejects_missing_extra_duplicate_and_mutated_members(self) -> None:
        mutations = {
            "missing": {
                name: value
                for name, value in self.payloads.items()
                if name != "SHA256SUMS.txt"
            },
            "extra": {**self.payloads, "untrusted.bin": b"extra"},
            "mutated": {**self.payloads, self.archive_name: b"wrong bytes"},
        }
        for name, payloads in mutations.items():
            with self.subTest(name=name):
                artifact = self.write_artifact(payloads)
                with self.assertRaises(ValueError):
                    MODULE.extract(
                        artifact,
                        self.record_path,
                        self.root / f"staged-{name}",
                    )
                artifact.unlink()
        artifact = self.write_artifact(duplicate=self.archive_name)
        with self.assertRaisesRegex(ValueError, "exact public candidate allowlist"):
            MODULE.extract(artifact, self.record_path, self.root / "staged-duplicate")

    def test_rejects_traversal_and_nested_members(self) -> None:
        for bad_name in ("../SHA256SUMS.txt", "nested/SHA256SUMS.txt"):
            payloads = dict(self.payloads)
            payloads.pop("SHA256SUMS.txt")
            payloads[bad_name] = b"archive checksum\n"
            artifact = self.write_artifact(payloads)
            with self.assertRaises(ValueError):
                MODULE.extract(
                    artifact,
                    self.record_path,
                    self.root / f"staged-{bad_name.replace('/', '-')}",
                )
            artifact.unlink()


if __name__ == "__main__":
    unittest.main()
