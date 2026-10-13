#!/usr/bin/env python3
"""Recreate the excluded pinned native source for this research handoff.

No model, build, installation or inference is run. Download requires --download.
Existing vendor trees are never overwritten.
"""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request


def sha(data):
    return hashlib.sha256(data).hexdigest()


def tree_hash(root):
    rows = []
    for p in sorted(root.rglob('*'), key=lambda p: p.relative_to(root).as_posix()):
        if p.is_symlink() or not (p.is_file() or p.is_dir()):
            raise SystemExit('Symlink or special entry refused in native source')
        if p.is_file():
            data = p.read_bytes()
            rows.append({'path': p.relative_to(root).as_posix(), 'SHA256': sha(data), 'bytes': len(data)})
    return sha(json.dumps(rows, separators=(',', ':')).encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--crate', type=pathlib.Path, help='Existing upstream 0.1.151 .crate archive')
    parser.add_argument('--download', action='store_true', help='Explicitly download the checksum-pinned upstream source')
    parser.add_argument('--verify', action='store_true', help='Only verify an existing reconstructed native tree')
    args = parser.parse_args()
    repo = pathlib.Path(__file__).resolve().parent.parent
    patch_root = repo / 'vendor/llama-cpp-sys-2-patches'
    manifest = json.loads((patch_root / 'manifest.json').read_text())
    destination = repo / 'vendor/llama-cpp-sys-2'
    if args.verify:
        if destination.is_symlink() or not destination.is_dir() or tree_hash(destination) != manifest['source_tree_SHA256']:
            raise SystemExit('Native source tree does not match the handoff manifest')
        print('Verified exact handoff native source tree; no build or model activity')
        return
    if os.path.lexists(destination):
        raise SystemExit('Existing native tree preserved; use --verify or a separate clean checkout')
    patch_file = patch_root / '0.1.151.patch'
    if sha(patch_file.read_bytes()) != manifest['patch_SHA256']:
        raise SystemExit('Native patch checksum mismatch')
    cargo_cache = pathlib.Path(os.environ.get('CARGO_HOME', str(pathlib.Path.home() / '.cargo'))) / 'registry/cache'
    archives = [args.crate] if args.crate else list(cargo_cache.glob('*/llama-cpp-sys-2-0.1.151.crate'))
    archive = next((p for p in archives if p.is_file() and sha(p.read_bytes()) == manifest['crate_SHA256']), None)
    if archive is None and not args.download:
        raise SystemExit('No checksum-matching cached crate; supply --crate or explicitly authorize --download')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.native-handoff-', dir=destination.parent) as temp:
        temp = pathlib.Path(temp)
        if archive is None:
            archive = temp / 'upstream.crate'
            with urllib.request.urlopen(manifest['crate_download'], timeout=60) as response:
                archive.write_bytes(response.read(manifest['crate_bytes'] + 1))
            if sha(archive.read_bytes()) != manifest['crate_SHA256']:
                raise SystemExit('Downloaded crate checksum mismatch')
        staged = temp / 'source'
        staged.mkdir()
        with tarfile.open(archive) as package:
            for member in package.getmembers():
                path = pathlib.PurePosixPath(member.name)
                if path.parts[0] != 'llama-cpp-sys-2-0.1.151' or '..' in path.parts or path.is_absolute():
                    raise SystemExit('Unexpected archive path')
                relative = pathlib.Path(*path.parts[1:])
                target = staged / relative
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                elif member.isfile():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(package.extractfile(member).read())
                    target.chmod(member.mode & 0o777)
                else:
                    raise SystemExit('Non-regular archive entry refused')
        subprocess.run(['patch', '--batch', '--forward', '-p1', '--input', str(patch_file)], cwd=staged, check=True)
        if tree_hash(staged) != manifest['source_tree_SHA256']:
            raise SystemExit('Reconstructed native tree checksum mismatch')
        # mkdir and exclusive file creates preserve any concurrent user entry.
        # A failed copy intentionally leaves its partial destination for review.
        destination.mkdir()
        for source in sorted(staged.rglob('*')):
            target = destination / source.relative_to(staged)
            if source.is_dir():
                target.mkdir()
            else:
                with target.open('xb') as output:
                    output.write(source.read_bytes())
                target.chmod(source.stat().st_mode & 0o777)
        if tree_hash(destination) != manifest['source_tree_SHA256']:
            raise SystemExit('Published native content mismatch; destination preserved for review')
    print('Recreated exact handoff native source; no build, model or installation activity')


if __name__ == '__main__':
    main()
