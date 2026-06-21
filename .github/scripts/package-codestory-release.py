#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import shutil
import tarfile
import tempfile
import zipfile
from pathlib import Path


def copy_required_file(root: Path, relative: str, destination_root: Path) -> None:
    source = root / relative
    if not source.is_file():
        raise FileNotFoundError(f"required release file is missing: {relative}")
    destination = destination_root / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def copy_required_dir(root: Path, relative: str, destination_root: Path) -> None:
    source = root / relative
    if not source.is_dir():
        raise FileNotFoundError(f"required release directory is missing: {relative}")
    destination = destination_root / relative
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination)


def archive_zip(source_dir: Path, archive_path: Path) -> None:
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(source_dir.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(source_dir.parent).as_posix())


def archive_tar_gz(source_dir: Path, archive_path: Path) -> None:
    with tarfile.open(archive_path, "w:gz") as archive:
        archive.add(source_dir, arcname=source_dir.name)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description="Package a CodeStory CLI release binary.")
    parser.add_argument("--version", required=True, help="Release version without v prefix.")
    parser.add_argument("--target", required=True, help="Asset target label.")
    parser.add_argument("--binary", required=True, help="Built codestory-cli binary path.")
    parser.add_argument("--out-dir", required=True, help="Directory for archive and checksum outputs.")
    parser.add_argument("--project-root", default=".", help="Repository root.")
    args = parser.parse_args()

    root = Path(args.project_root).resolve()
    binary = Path(args.binary).resolve()
    if not binary.is_file():
        raise FileNotFoundError(f"binary does not exist: {binary}")

    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    archive_base = f"codestory-cli-v{args.version}-{args.target}"
    archive_ext = ".zip" if "windows" in args.target.lower() else ".tar.gz"
    archive_path = out_dir / f"{archive_base}{archive_ext}"

    with tempfile.TemporaryDirectory(prefix="codestory-release-", dir=out_dir) as temp_dir:
        stage_root = Path(temp_dir) / archive_base
        stage_root.mkdir(parents=True)

        binary_name = "codestory-cli.exe" if binary.suffix.lower() == ".exe" else "codestory-cli"
        shutil.copy2(binary, stage_root / binary_name)

        copy_required_file(root, "README.md", stage_root)
        copy_required_file(root, "LICENSE", stage_root)
        copy_required_file(root, "docs/usage.md", stage_root)
        copy_required_dir(root, "plugins/codestory/skills/codestory-grounding", stage_root)

        if archive_ext == ".zip":
            archive_zip(stage_root, archive_path)
        else:
            archive_tar_gz(stage_root, archive_path)

    checksum = sha256_file(archive_path)
    checksum_line = f"{checksum}  {archive_path.name}\n"
    checksum_path = out_dir / f"{archive_path.name}.sha256"
    checksum_path.write_text(checksum_line, encoding="utf-8", newline="\n")
    (out_dir / "SHA256SUMS.txt").write_text(checksum_line, encoding="utf-8", newline="\n")

    print(f"archive={archive_path}")
    print(f"checksum={checksum_path}")


if __name__ == "__main__":
    main()
