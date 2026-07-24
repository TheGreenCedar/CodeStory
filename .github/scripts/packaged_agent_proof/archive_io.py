"""Safe package archive inspection and extraction."""

from __future__ import annotations

import stat
import tarfile
import zipfile
from pathlib import Path

from .foundation import ProofFailure, require


def expected_archive_digest(checksum_file: Path, archive: Path) -> str:
    lines = checksum_file.read_text(encoding="utf-8").splitlines()
    records: dict[str, str] = {}
    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 2 and len(parts[0]) == 64:
            records[parts[-1].lstrip("*")] = parts[0].lower()
        elif len(parts) == 1 and len(parts[0]) == 64:
            records[archive.name] = parts[0].lower()
    require(archive.name in records, f"checksum file does not name {archive.name}")
    return records[archive.name]


def safe_target(root: Path, name: str) -> Path:
    target = (root / name).resolve()
    require(
        target.is_relative_to(root.resolve()),
        f"archive member escapes extraction root: {name}",
    )
    return target


def unpack_archive(archive: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                safe_target(destination, member.filename)
            handle.extractall(destination)
        return
    if tarfile.is_tarfile(archive):
        with tarfile.open(archive) as handle:
            members = handle.getmembers()
            for member in members:
                safe_target(destination, member.name)
                require(
                    not member.issym() and not member.islnk(),
                    f"archive contains link: {member.name}",
                )
            handle.extractall(destination, members=members)
        return
    raise ProofFailure(f"unsupported archive format: {archive}")


def find_cli(root: Path) -> Path:
    names = {"codestory-cli", "codestory-cli.exe"}
    matches = [
        path for path in root.rglob("*") if path.is_file() and path.name in names
    ]
    require(
        len(matches) == 1,
        f"archive must contain exactly one native CodeStory executable; found {len(matches)}",
    )
    cli = matches[0]
    cli.chmod(cli.stat().st_mode | stat.S_IXUSR)
    return cli
