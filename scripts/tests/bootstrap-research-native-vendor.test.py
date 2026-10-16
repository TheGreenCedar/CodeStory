#!/usr/bin/env python3
"""Exercise source reconstruction without network access or native builds."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
BOOTSTRAP = REPO / 'scripts/bootstrap-research-native-vendor.py'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def content_digest(files):
    rows = [{'path': name, 'SHA256': digest(data), 'bytes': len(data)}
            for name, data in sorted(files.items())]
    return digest(json.dumps(rows, separators=(',', ':')).encode())


class NativeBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='native-bootstrap-test-')
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        script = self.repo / 'scripts/bootstrap-research-native-vendor.py'
        script.parent.mkdir()
        shutil.copyfile(BOOTSTRAP, script)
        self.script = script
        self.patch_root = self.repo / 'vendor/llama-cpp-sys-2-patches'
        self.patch_root.mkdir(parents=True)
        self.destination = self.repo / 'vendor/llama-cpp-sys-2'
        self.archive = self.repo / 'upstream.crate'
        self.before = {'Cargo.toml': b'[package]\nname = "native-fixture"\n', 'source.c': b'old\n'}
        self.after = {**self.before, 'source.c': b'new\n', 'added.h': b'guard\n'}
        patch = (b'--- a/source.c\n+++ b/source.c\n@@ -1 +1 @@\n-old\n+new\n'
                 b'--- /dev/null\n+++ b/added.h\n@@ -0,0 +1 @@\n+guard\n')
        (self.patch_root / '0.1.151.patch').write_bytes(patch)
        self.write_archive(self.before)
        self.manifest = {'crate_SHA256': digest(self.archive.read_bytes()),
                         'patch_SHA256': digest(patch),
                         'source_tree_SHA256': content_digest(self.after),
                         'crate_bytes': self.archive.stat().st_size,
                         'crate_download': 'http://127.0.0.1:1/must-not-download'}
        self.write_manifest()
        self.env = {**os.environ, 'CARGO_HOME': str(self.repo / 'empty-cargo')}

    def write_archive(self, files, extra=None):
        with tarfile.open(self.archive, 'w:gz') as archive:
            for name, data in files.items():
                member = tarfile.TarInfo('llama-cpp-sys-2-0.1.151/' + name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
            if extra:
                archive.addfile(extra)

    def write_manifest(self):
        (self.patch_root / 'manifest.json').write_text(json.dumps(self.manifest))

    def run_bootstrap(self, *args):
        return subprocess.run([sys.executable, str(self.script), *map(str, args)],
                              env=self.env, capture_output=True, text=True)

    def reconstruct(self):
        result = self.run_bootstrap('--ensure', '--crate', self.archive)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual({p.relative_to(self.destination).as_posix(): p.read_bytes()
                          for p in self.destination.rglob('*') if p.is_file()}, self.after)

    def test_reconstruct_and_repeat_without_archive_or_download(self):
        self.reconstruct()
        self.archive.unlink()
        result = self.run_bootstrap('--ensure', '--download')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Verified exact', result.stdout)
        result = self.run_bootstrap('--verify')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_warm_ensure_requires_patch_identity_but_verify_only_checks_tree(self):
        self.reconstruct()
        patch_file = self.patch_root / '0.1.151.patch'
        original_patch = patch_file.read_bytes()
        for mutation in ('corrupt', 'missing'):
            with self.subTest(mutation=mutation):
                patch_file.write_bytes(original_patch)
                if mutation == 'corrupt':
                    patch_file.write_bytes(b'corrupt patch')
                else:
                    patch_file.unlink()
                result = self.run_bootstrap('--ensure', '--download')
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn('Native patch checksum mismatch', result.stderr)
                self.assertEqual({p.relative_to(self.destination).as_posix(): p.read_bytes()
                                  for p in self.destination.rglob('*') if p.is_file()}, self.after)
                verified = self.run_bootstrap('--verify')
                self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_explicit_download_from_local_archive_and_checksum_rejection(self):
        self.manifest['crate_download'] = self.archive.as_uri()
        self.write_manifest()
        result = self.run_bootstrap('--ensure', '--download')
        self.assertEqual(result.returncode, 0, result.stderr)
        # Retry against a bad downloaded payload in a fresh destination.
        shutil.rmtree(self.destination)
        self.archive.write_bytes(b'wrong archive')
        result = self.run_bootstrap('--ensure', '--download')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Downloaded crate checksum mismatch', result.stderr)
        self.assertFalse(self.destination.exists())

    def test_patch_application_failure_publishes_nothing(self):
        self.write_archive({**self.before, 'source.c': b'unrelated source\n'})
        self.manifest['crate_SHA256'] = digest(self.archive.read_bytes())
        self.write_manifest()
        result = self.run_bootstrap('--ensure', '--crate', self.archive)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('patch does not apply', result.stderr)
        self.assertFalse(self.destination.exists())

    def test_corrupt_existing_tree_is_preserved_and_refused(self):
        self.reconstruct()
        (self.destination / 'source.c').write_bytes(b'private edit\n')
        result = self.run_bootstrap('--ensure', '--download')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('does not match', result.stderr)
        self.assertEqual((self.destination / 'source.c').read_bytes(), b'private edit\n')

    def test_no_download_without_opt_in_and_bad_inputs_publish_nothing(self):
        for failure in ('missing archive', 'wrong archive', 'wrong patch', 'wrong postimage'):
            with self.subTest(failure=failure):
                manifest = dict(self.manifest)
                args = ['--ensure', '--crate', self.archive]
                if failure == 'missing archive':
                    args = ['--ensure']
                elif failure == 'wrong archive':
                    manifest['crate_SHA256'] = '0' * 64
                elif failure == 'wrong patch':
                    manifest['patch_SHA256'] = '0' * 64
                else:
                    manifest['source_tree_SHA256'] = '0' * 64
                (self.patch_root / 'manifest.json').write_text(json.dumps(manifest))
                result = self.run_bootstrap(*args)
                self.assertNotEqual(result.returncode, 0)
                expected = {'missing archive': 'No checksum-matching cached crate',
                            'wrong archive': 'No checksum-matching cached crate',
                            'wrong patch': 'Native patch checksum mismatch',
                            'wrong postimage': 'Reconstructed native tree checksum mismatch'}
                self.assertIn(expected[failure], result.stderr)
                self.assertFalse(self.destination.exists())
                self.assertNotIn('urlopen', result.stderr)

    def test_archive_symlink_and_traversal_are_refused(self):
        for kind in ('symlink', 'traversal'):
            with self.subTest(kind=kind):
                member = tarfile.TarInfo('llama-cpp-sys-2-0.1.151/' +
                                        ('escape' if kind == 'symlink' else '../escape'))
                if kind == 'symlink':
                    member.type = tarfile.SYMTYPE
                    member.linkname = '/tmp'
                self.write_archive(self.before, member)
                self.manifest['crate_SHA256'] = digest(self.archive.read_bytes())
                self.write_manifest()
                result = self.run_bootstrap('--ensure', '--crate', self.archive)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('refused' if kind == 'symlink' else 'Unexpected archive path', result.stderr)
                self.assertFalse(self.destination.exists())

    def test_existing_symlink_is_never_followed(self):
        private = self.repo / 'private'
        private.mkdir()
        (private / 'source.c').write_bytes(b'private\n')
        self.destination.symlink_to(private, target_is_directory=True)
        result = self.run_bootstrap('--ensure', '--download')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((private / 'source.c').read_bytes(), b'private\n')


if __name__ == '__main__':
    unittest.main()
