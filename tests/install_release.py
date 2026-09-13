#!/usr/bin/env python3
"""Release install checks: fake transport, real archives, isolated destinations."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.prefix = self.root / 'prefix with spaces'
        self.bin = self.prefix / 'bin'
        self.bin.mkdir(parents=True)
        (self.bin / 'yupana').write_bytes(b'old binary')
        (self.bin / 'hank').symlink_to('yupana')
        self.archive = self.root / 'yupana-v0.8.0-x86_64-linux-gnu.tar.gz'
        curl = self.tools / 'curl'
        curl.write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
url = next(a for a in args if a.startswith('https://'))
assert url.startswith('https://github.com/scbrown/yupana/releases/download/v0.8.0/')
if os.environ.get('FAIL_DOWNLOAD'): sys.exit(22)
source = pathlib.Path(os.environ['FIXTURES']) / url.rsplit('/', 1)[-1]
shutil.copyfile(source, args[args.index('--output') + 1])
''')
        curl.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ['PATH'],
                        FIXTURES=str(self.root), YUPANA_INSTALL_ROOT=str(self.prefix))

    def make_archive(self, version='0.8.0', missing='', member='yupana'):
        binary = (f'#!/bin/sh\nif [ "$1" = --version ]; then echo "yupana {version}"; exit 0; fi\n'
                  f'if [ "$1" = "{missing}" ]; then exit 1; fi\n'
                  'case "$1" in exemplar|verifier|verdicts) test "$2" = --help;; *) exit 2;; esac\n').encode()
        with tarfile.open(self.archive, 'w:gz') as archive:
            info = tarfile.TarInfo(member)
            info.size, info.mode = len(binary), 0o755
            archive.addfile(info, io.BytesIO(binary))
            alias = tarfile.TarInfo('hank')
            alias.type, alias.linkname = tarfile.SYMTYPE, 'yupana'
            archive.addfile(alias)
        self.sums = Path(str(self.archive) + '.sha256')
        self.sums.write_text(hashlib.sha256(self.archive.read_bytes()).hexdigest() + '  ' + self.archive.name + '\n')
        return binary

    def run_install(self, *args):
        return subprocess.run([str(ROOT / 'scripts/install-release.sh'), *(args or ('v0.8.0',))],
                              env=self.env, capture_output=True, text=True, check=False)

    def assert_unchanged(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual((self.bin / 'yupana').read_bytes(), b'old binary')
        self.assertEqual(os.readlink(self.bin / 'hank'), 'yupana')
        self.assertEqual(list(self.bin.glob('.yupana-candidate.*')), [])

    def test_publishes_exact_archive_bytes_without_cargo(self):
        expected = self.make_archive()
        cargo = self.tools / 'cargo'
        cargo.write_text('#!/bin/sh\nexit 99\n')
        cargo.chmod(0o755)
        result = self.run_install('0.8.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / 'yupana').read_bytes(), expected)
        self.assertEqual((self.bin / 'hank').resolve(), self.bin / 'yupana')
        self.assertIn('Installed release v0.8.0', result.stdout)

    def test_checksum_mismatch_preserves_install(self):
        self.make_archive()
        self.sums.write_text('0' * 64 + '  ' + self.archive.name + '\n')
        self.assert_unchanged(self.run_install())

    def test_foreign_checksum_filename_preserves_install(self):
        self.make_archive()
        self.sums.write_text(hashlib.sha256(self.archive.read_bytes()).hexdigest() + '  /etc/passwd\n')
        self.assert_unchanged(self.run_install())

    def test_failed_download_preserves_install(self):
        self.make_archive()
        self.env['FAIL_DOWNLOAD'] = '1'
        self.assert_unchanged(self.run_install())

    def test_wrong_version_or_missing_capability_preserves_install(self):
        for version, missing in [('0.7.0', ''), ('0.8.0', 'verdicts')]:
            with self.subTest(version=version, missing=missing):
                self.make_archive(version, missing)
                self.assert_unchanged(self.run_install())

    def test_archive_path_escape_is_rejected(self):
        self.make_archive(member='../escaped')
        self.assert_unchanged(self.run_install())
        self.assertFalse((self.root / 'escaped').exists())

    def test_source_installer_identifies_source_and_refuses_dirty_shared_checkout(self):
        repo = self.root / 'repo'
        scripts = repo / 'scripts'
        scripts.mkdir(parents=True)
        shutil.copyfile(ROOT / 'scripts/install-local.sh', scripts / 'install-local.sh')
        def git(*args):
            return subprocess.run(['git', '-C', str(repo), *args], check=True,
                                  capture_output=True, text=True)
        git('init')
        git('add', '.')
        git('-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '-m', 'fixture')
        git('worktree', 'add', '-b', 'sibling', str(self.root / 'sibling'))
        (repo / 'uncommitted').write_text('in flight')
        result = subprocess.run(['bash', str(scripts / 'install-local.sh')], env=self.env,
                                capture_output=True, text=True, check=False)
        self.assert_unchanged(result)
        self.assertIn('CURRENT CHECKOUT', result.stdout)
        self.assertIn('state: DIRTY', result.stdout)
        self.assertIn(git('rev-parse', 'HEAD').stdout.strip(), result.stdout)
        self.assertIn('refusing a dirty shared checkout', result.stderr)
        help_result = subprocess.run(['bash', str(scripts / 'install-local.sh'), '--help'],
                                     capture_output=True, text=True, check=True)
        self.assertIn('CURRENT CHECKOUT', help_result.stdout)


if __name__ == '__main__':
    unittest.main()
