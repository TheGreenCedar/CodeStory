#!/usr/bin/env python3
"""Extract one authenticated Actions artifact into a public candidate payload."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import tempfile
import zipfile
from pathlib import Path

SHA = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
TARGET = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
PORTABLE_NAME = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9._+-]*[A-Za-z0-9])?$")
RESERVED_NAME = re.compile(
    r"^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$",
    re.IGNORECASE,
)
RECORD_SCHEMA = "codestory-candidate-archive-store/v1"


def fail(message: str) -> None:
    raise ValueError(message)


def exact_keys(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        fail(f"{label} keys changed")
    return value


def positive_bytes(value: object, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        fail(f"{label} must be a positive integer")
    return value


def digest(value: object, pattern: re.Pattern[str], label: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        fail(f"{label} is invalid")
    return value


def portable_name(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or not PORTABLE_NAME.fullmatch(value)
        or RESERVED_NAME.fullmatch(value)
        or "/" in value
        or "\\" in value
    ):
        fail(f"{label} must be a portable simple filename")
    return value


def descriptor(value: object, role: str, expected_path: str) -> tuple[str, int, str]:
    item = exact_keys(
        value,
        {"role", "relative_path", "bytes", "sha256"},
        f"{role} descriptor",
    )
    if item["role"] != role or item["relative_path"] != expected_path:
        fail(f"{role} descriptor path changed")
    return (
        expected_path,
        positive_bytes(item["bytes"], f"{role} bytes"),
        digest(item["sha256"], SHA256, f"{role} SHA-256"),
    )


def load_record(record_path: Path) -> dict[str, tuple[int, str]]:
    record = exact_keys(
        json.loads(record_path.read_text(encoding="utf-8")),
        {"schema", "repository", "source", "target", "archive", "companions"},
        "candidate archive record",
    )
    if record["schema"] != RECORD_SCHEMA:
        fail("candidate archive record schema changed")
    if (
        not isinstance(record["repository"], str)
        or record["repository"].count("/") != 1
    ):
        fail("candidate repository changed")
    source = exact_keys(record["source"], {"commit", "tree"}, "candidate source")
    digest(source["commit"], SHA, "candidate source SHA")
    digest(source["tree"], SHA, "candidate source tree")
    digest(record["target"], TARGET, "candidate target")
    archive = exact_keys(
        record["archive"],
        {"name", "relative_path", "bytes", "sha256"},
        "candidate archive",
    )
    archive_name = portable_name(archive["name"], "candidate archive name")
    if archive["relative_path"] != archive_name:
        fail("candidate archive path changed")
    expected = {
        archive_name: (
            positive_bytes(archive["bytes"], "candidate archive bytes"),
            digest(archive["sha256"], SHA256, "candidate archive SHA-256"),
        )
    }
    companions = record["companions"]
    if not isinstance(companions, list) or len(companions) != 2:
        fail("candidate record must retain exactly the two public checksum files")
    by_role = {
        item.get("role"): item
        for item in companions
        if isinstance(item, dict)
    }
    if set(by_role) != {"archive_checksum", "checksum_manifest"}:
        fail("candidate record companions must remain public-only")
    archive_checksum = descriptor(
        by_role["archive_checksum"],
        "archive_checksum",
        f"{archive_name}.sha256",
    )
    checksum_manifest = descriptor(
        by_role["checksum_manifest"],
        "checksum_manifest",
        "SHA256SUMS.txt",
    )
    if archive_checksum[1:] != checksum_manifest[1:]:
        fail("per-candidate checksum files must retain the same checksum line")
    for path, size, sha256 in (archive_checksum, checksum_manifest):
        expected[path] = (size, sha256)
    return expected


def zip_entry_is_regular(info: zipfile.ZipInfo) -> bool:
    unix_mode = info.external_attr >> 16
    file_type = stat.S_IFMT(unix_mode)
    return file_type in (0, stat.S_IFREG)


def extract(artifact: Path, record: Path, output: Path) -> None:
    expected = load_record(record)
    output = output.resolve()
    if output.exists():
        fail(f"candidate staging output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(
        tempfile.mkdtemp(
            prefix=f".{output.name}.partial-",
            dir=output.parent,
        )
    )
    try:
        with zipfile.ZipFile(artifact) as bundle:
            members = bundle.infolist()
            names = [member.filename for member in members]
            if len(names) != len(set(names)) or set(names) != set(expected):
                fail("Actions artifact does not contain the exact public candidate allowlist")
            for member in members:
                if (
                    member.is_dir()
                    or member.flag_bits & 0x1
                    or not zip_entry_is_regular(member)
                    or portable_name(member.filename, "Actions artifact member")
                    != member.filename
                ):
                    fail("Actions artifact members must be unencrypted regular root files")
                expected_size, expected_sha256 = expected[member.filename]
                if member.file_size != expected_size:
                    fail(f"Actions artifact member size changed: {member.filename}")
                destination = temporary / member.filename
                measured = hashlib.sha256()
                written = 0
                with bundle.open(member) as source, destination.open("xb") as target:
                    while chunk := source.read(1024 * 1024):
                        written += len(chunk)
                        if written > expected_size:
                            fail(f"Actions artifact member exceeded its size: {member.filename}")
                        measured.update(chunk)
                        target.write(chunk)
                    target.flush()
                    os.fsync(target.fileno())
                if written != expected_size or measured.hexdigest() != expected_sha256:
                    fail(f"Actions artifact member identity changed: {member.filename}")
        temporary.rename(output)
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact", required=True, type=Path)
    parser.add_argument("--record", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    extract(args.artifact, args.record, args.out)


if __name__ == "__main__":
    main()
