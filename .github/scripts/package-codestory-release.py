#!/usr/bin/env python3
from __future__ import annotations

import argparse
import gzip
import hashlib
import shutil
import stat
import tarfile
import tempfile
import zipfile
from pathlib import Path

NORMALIZED_MTIME = 315532800  # 1980-01-01T00:00:00Z, valid for zip and tar.


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
                info = zipfile.ZipInfo(
                    path.relative_to(source_dir.parent).as_posix(),
                    date_time=(1980, 1, 1, 0, 0, 0),
                )
                info.compress_type = zipfile.ZIP_DEFLATED
                info.create_system = 3
                info.external_attr = normalized_file_mode(path) << 16
                archive.writestr(info, path.read_bytes())


def archive_tar_gz(source_dir: Path, archive_path: Path) -> None:
    with archive_path.open("wb") as raw:
        with gzip.GzipFile(
            filename="", mode="wb", fileobj=raw, mtime=NORMALIZED_MTIME
        ) as gzip_file:
            with tarfile.open(fileobj=gzip_file, mode="w") as archive:
                for path in [source_dir, *sorted(source_dir.rglob("*"))]:
                    info = archive.gettarinfo(
                        str(path), arcname=path.relative_to(source_dir.parent).as_posix()
                    )
                    info.mtime = NORMALIZED_MTIME
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mode = 0o755 if path.is_dir() else normalized_file_mode(path)
                    if path.is_file():
                        with path.open("rb") as handle:
                            archive.addfile(info, handle)
                    else:
                        archive.addfile(info)


def normalized_file_mode(path: Path) -> int:
    mode = path.stat().st_mode
    if path.suffix.lower() == ".exe" or mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH):
        return 0o755
    return 0o644


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def package_release(
    version: str, target: str, binary: Path, out_dir: Path, root: Path
) -> Path:
    if not binary.is_file():
        raise FileNotFoundError(f"binary does not exist: {binary}")

    out_dir.mkdir(parents=True, exist_ok=True)

    archive_base = f"codestory-cli-v{version}-{target}"
    archive_ext = ".zip" if "windows" in target.lower() else ".tar.gz"
    archive_path = out_dir / f"{archive_base}{archive_ext}"

    with tempfile.TemporaryDirectory(prefix="codestory-release-", dir=out_dir) as temp_dir:
        stage_root = Path(temp_dir) / archive_base
        stage_root.mkdir(parents=True)

        binary_name = "codestory-cli.exe" if binary.suffix.lower() == ".exe" else "codestory-cli"
        shutil.copy2(binary, stage_root / binary_name)

        copy_required_file(root, "README.md", stage_root)
        copy_required_file(root, "LICENSE", stage_root)
        copy_required_file(root, "docs/glossary.md", stage_root)
        copy_required_dir(root, "docs/users", stage_root)
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
    return archive_path


def run_self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-package-self-test-") as temp_dir:
        temp_root = Path(temp_dir)
        repo_root = temp_root / "repo"
        (repo_root / "docs/users").mkdir(parents=True)
        (repo_root / "plugins/codestory/skills/codestory-grounding").mkdir(parents=True)
        (repo_root / "README.md").write_text("readme\n", encoding="utf-8")
        (repo_root / "LICENSE").write_text("license\n", encoding="utf-8")
        (repo_root / "docs/glossary.md").write_text("glossary\n", encoding="utf-8")
        (repo_root / "docs/users/guide.md").write_text("guide\n", encoding="utf-8")
        (repo_root / "plugins/codestory/skills/codestory-grounding/SKILL.md").write_text(
            "skill\n", encoding="utf-8"
        )

        linux_binary = temp_root / "codestory-cli"
        linux_binary.write_text("#!/bin/sh\n", encoding="utf-8")
        linux_binary.chmod(0o755)
        windows_binary = temp_root / "codestory-cli.exe"
        windows_binary.write_bytes(b"exe\n")

        for target, binary in [
            ("linux-x64", linux_binary),
            ("windows-x64", windows_binary),
        ]:
            first = package_release("0.0.0", target, binary, temp_root / f"{target}-1", repo_root)
            second = package_release("0.0.0", target, binary, temp_root / f"{target}-2", repo_root)
            first_digest = sha256_file(first)
            second_digest = sha256_file(second)
            if first_digest != second_digest:
                raise AssertionError(
                    f"{target} package checksum changed across identical inputs: "
                    f"{first_digest} != {second_digest}"
                )

    print("package self-test passed")


def main() -> None:
    parser = argparse.ArgumentParser(description="Package a CodeStory CLI release binary.")
    parser.add_argument("--self-test", action="store_true", help="Run package-twice proof.")
    parser.add_argument("--version", help="Release version without v prefix.")
    parser.add_argument("--target", help="Asset target label.")
    parser.add_argument("--binary", help="Built codestory-cli binary path.")
    parser.add_argument("--out-dir", help="Directory for archive and checksum outputs.")
    parser.add_argument("--project-root", default=".", help="Repository root.")
    args = parser.parse_args()

    if args.self_test:
        run_self_test()
        return

    for required in ["version", "target", "binary", "out_dir"]:
        if getattr(args, required) is None:
            parser.error(f"--{required.replace('_', '-')} is required unless --self-test is used")

    root = Path(args.project_root).resolve()
    binary = Path(args.binary).resolve()
    out_dir = Path(args.out_dir).resolve()
    archive_path = package_release(args.version, args.target, binary, out_dir, root)
    checksum_path = out_dir / f"{archive_path.name}.sha256"

    print(f"archive={archive_path}")
    print(f"checksum={checksum_path}")


if __name__ == "__main__":
    main()
