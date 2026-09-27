#!/usr/bin/env python3
"""Read selected JSON members from a digest-authenticated Actions container."""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import stat
import zipfile


def read_members(archive: Path, digest: str, wanted: list[str]) -> dict[str, str]:
    raw = archive.read_bytes()
    if "sha256:" + hashlib.sha256(raw).hexdigest() != digest:
        raise ValueError("Actions artifact bytes differ from the API digest")
    with zipfile.ZipFile(archive) as bundle:
        entries = bundle.infolist()
        names = [entry.filename for entry in entries]
        if len(entries) > 1000 or len(names) != len(set(names)):
            raise ValueError("Actions artifact contains excessive or duplicate members")
        if sum(entry.file_size for entry in entries) > 64 * 1024 * 1024:
            raise ValueError("release evidence artifact exceeds the JSON evidence bound")
        for entry in entries:
            name = entry.filename
            parts = PurePosixPath(name).parts
            kind = stat.S_IFMT(entry.external_attr >> 16)
            if (not parts or name.startswith("/") or ".." in parts
                    or "\\" in name or ":" in name or entry.flag_bits & 1
                    or kind not in (0, stat.S_IFREG, stat.S_IFDIR)):
                raise ValueError("Actions artifact contains an unsafe member")
        result = {}
        for name in wanted:
            if name not in names:
                raise ValueError(f"Actions artifact is missing {name}")
            entry = bundle.getinfo(name)
            if entry.is_dir() or entry.file_size > 16 * 1024 * 1024:
                raise ValueError("selected release evidence is not a bounded file")
            text = bundle.read(entry).decode("utf-8")
            json.loads(text)
            result[name] = text
        return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--member", action="append", required=True)
    args = parser.parse_args()
    print(json.dumps(read_members(args.archive, args.sha256, args.member)))
