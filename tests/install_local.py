#!/usr/bin/env python3
"""Adversarial installer tests; every binary, build and prefix lives in a temp tree."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FAKE_CARGO = r'''#!/usr/bin/env python3
import json, os, pathlib, sys, time
args = sys.argv[1:]
target = pathlib.Path(args[args.index('--target-dir') + 1])
assert '--all-features' in args and '--locked' in args
assert args[0] == 'build' and '--release' in args
mode = os.environ.get('FAKE_MODE', 'ok')
marker = os.environ.get('SOURCE_MARKER', 'new-source')
with open(os.environ['BUILD_LOG'], 'a') as log:
    log.write(json.dumps({'target': str(target), 'args': args}) + '\n')
if mode == 'build-fail': sys.exit(17)
if mode == 'missing': sys.exit(0)
# Simulate a wrapper overwriting the environment target. The explicit flag wins.
shared = pathlib.Path(os.environ['CARGO_TARGET_DIR'])
shared.mkdir(exist_ok=True)
(shared / 'yupana').write_text('another worktree overwrote the shared artifact')
release = target / 'release'
release.mkdir()
binary = release / 'yupana'
value = 'stale-same-version' if mode == 'stale' else marker
binary.write_text('#!/usr/bin/env python3\nimport sys\nprint("yupana 0.6.5" if sys.argv[1] == "--version" else '+repr(value)+')\n')
binary.chmod(0o755)
contract = release / 'install-contract'
contract.write_text('#!/usr/bin/env python3\nimport subprocess, sys, time\ntime.sleep(0.05)\nactual=subprocess.check_output([sys.argv[1], "--proof"], text=True).strip()\nassert actual == '+repr(marker)+', "candidate CLI differs from source"\nprint("Verified source contract")\n')
contract.chmod(0o755)
if mode in ('outside', 'symlink'):
    outside = shared / 'foreign'
    outside.write_bytes(binary.read_bytes()); outside.chmod(0o755)
    if mode == 'outside': binary = outside
    else:
        binary.unlink(); binary.symlink_to(outside)
for name, file in [('yupana', binary), ('install-contract', contract)]:
    if mode == 'no-contract' and name == 'install-contract': continue
    record = {'reason':'compiler-artifact', 'target':{'name':name},'executable':str(file)}
    print(json.dumps(record))
    if mode == 'duplicate' and name == 'yupana': print(json.dumps(record))
'''


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='yupana-install-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.prefix = self.root / 'prefix with spaces'
        self.builds = self.root / 'builds'
        self.bin = self.prefix / 'bin'
        self.bin.mkdir(parents=True)
        self.old = self.bin / 'yupana'
        self.old.write_text('old installed binary\n')
        self.before = self.old.read_bytes()
        (self.bin / 'hank').symlink_to('yupana')
        cargo = self.root / 'cargo'
        cargo.write_text(FAKE_CARGO)
        cargo.chmod(0o755)
        self.env = dict(os.environ, YUPANA_INSTALL_ROOT=str(self.prefix),
                        YUPANA_INSTALL_BUILD_ROOT=str(self.builds), CARGO_BIN=str(cargo),
                        CARGO_TARGET_DIR=str(self.root / 'shared'), BUILD_LOG=str(self.root / 'build.log'))

    def run_install(self, mode='ok'):
        return subprocess.run([str(ROOT / 'scripts/install-local.sh')],
                              env=dict(self.env, FAKE_MODE=mode), cwd=self.root,
                              capture_output=True, text=True, check=False)

    def assert_clean(self):
        self.assertEqual(list(self.builds.glob('build.*')), [])
        self.assertEqual(list(self.bin.glob('.yupana-candidate.*')), [])
        self.assertEqual(list(self.bin.glob('.hank-yupana.*')), [])

    def test_installs_private_build_despite_shared_artifact_clobber(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('SHA256:', result.stdout)
        self.assertEqual(subprocess.check_output([str(self.old), '--proof'], text=True).strip(), 'new-source')
        self.assertEqual((self.bin / 'hank').resolve(), self.old)
        log = json.loads((self.root / 'build.log').read_text())
        self.assertNotEqual(log['target'], self.env['CARGO_TARGET_DIR'])
        self.assert_clean()

    def test_refusals_preserve_old_binary_and_alias(self):
        for mode in ('stale', 'build-fail', 'missing', 'outside', 'symlink', 'duplicate', 'no-contract'):
            with self.subTest(mode=mode):
                result = self.run_install(mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('Installed ', result.stdout)
                self.assertEqual(self.old.read_bytes(), self.before)
                self.assertEqual(os.readlink(self.bin / 'hank'), 'yupana')
                self.assert_clean()

    def test_copy_corruption_is_caught_before_publication(self):
        tools = self.root / 'tools'
        tools.mkdir()
        install = tools / 'install'
        install.write_text('#!/usr/bin/env python3\nimport sys\nfrom pathlib import Path\nPath(sys.argv[-1]).write_text("corrupted copy")\n')
        install.chmod(0o755)
        self.env['PATH'] = str(tools) + os.pathsep + os.environ['PATH']
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.old.read_bytes(), self.before)
        self.assert_clean()

    def test_concurrent_installers_use_distinct_targets_and_publish_whole_binaries(self):
        commands = []
        for marker in ('source-a', 'source-b'):
            commands.append(subprocess.Popen([str(ROOT / 'scripts/install-local.sh')],
                env=dict(self.env, SOURCE_MARKER=marker), cwd=self.root,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True))
        def reap():
            for proc in commands:
                if proc.poll() is None:
                    proc.kill()
                proc.wait()
        self.addCleanup(reap)
        for proc in commands:
            out, err = proc.communicate(timeout=30)
            self.assertEqual(proc.returncode, 0, out + err)
        targets = [json.loads(line)['target'] for line in (self.root / 'build.log').read_text().splitlines()]
        self.assertEqual(len(set(targets)), 2)
        actual = subprocess.check_output([str(self.old), '--proof'], text=True).strip()
        self.assertIn(actual, ('source-a', 'source-b'))
        self.assertEqual((self.bin / 'hank').resolve(), self.old)
        self.assert_clean()


if __name__ == '__main__':
    unittest.main()
