#!/usr/bin/env python3
"""Run the actual publisher against an isolated release/latest transport model."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
TAG = 'v1.2.3'
PLATFORMS = ('x86_64-linux-gnu', 'aarch64-apple-darwin', 'x86_64-apple-darwin')

FAKE_GH = '''#!/usr/bin/env python3
import json, os, pathlib, sys
p = pathlib.Path(os.environ['RELEASE_STATE'])
s = json.loads(p.read_text())
a = sys.argv[1:]
release = s['release']
def record(kind):
    r = s['release']
    latest = r if r and not r['isDraft'] and not r['isPrerelease'] else s['previous']
    s['events'].append({'kind': kind, 'latest': json.loads(json.dumps(latest)), 'args': a})
    p.write_text(json.dumps(s))
if a[0] == 'api' and a[1].endswith('/releases/latest'):
    r = release if release and not release['isDraft'] and not release['isPrerelease'] else s['previous']
    print(json.dumps(r)); sys.exit(0)
assert a[:1] == ['release'], a
verb, tag = a[1:3]
if verb == 'view':
    if release is None: sys.exit(1)
    if '--jq' in a:
        assert a[a.index('--jq') + 1] == '.assets[].name', a
        print('\\n'.join(v['name'] for v in release['assets']))
    elif '--json' in a:
        shown = dict(release)
        if os.environ.get('BAD_METADATA'): shown.pop('isDraft')
        print(json.dumps(shown))
    else: print(tag)
elif verb == 'create':
    if release is not None: sys.exit(1)
    s['release'] = {'tagName': tag, 'isDraft': '--draft' in a, 'isPrerelease': False, 'assets': []}
    record('create')
elif verb == 'upload':
    assert release is not None
    s['uploads'] = s.get('uploads', 0) + 1
    files = [pathlib.Path(v) for v in a[3:] if not v.startswith('--')]
    fail = os.environ.get('FAIL_UPLOAD') or (os.environ.get('FAIL_FIRST_UPLOAD') and s['uploads'] == 1)
    if fail: files = files[:1]
    if os.environ.get('PARTIAL_SUCCESS'): files = files[:-1]
    for f in files:
        release['assets'] = [v for v in release['assets'] if v['name'] != f.name]
        release['assets'].append({'name': f.name, 'size': 0 if os.environ.get('BAD_SIZE') else f.stat().st_size,
                                 'state': 'new' if os.environ.get('BAD_STATE') else 'uploaded'})
        record('asset')
    record('upload')
    if fail: sys.exit(1)
elif verb == 'edit':
    assert release is not None
    if '--draft=false' in a: release['isDraft'] = False
    record('publish')
else: raise AssertionError(a)
'''


class PublicationTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.assets = self.root / 'assets'
        self.assets.mkdir()
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        for name, content in [('gh', FAKE_GH), ('sleep', '#!/bin/sh\nexit 0\n')]:
            tool = self.tools / name
            tool.write_text(content)
            tool.chmod(0o755)
        self.state = self.root / 'state.json'
        self.state.write_text(json.dumps({'release': None, 'events': [], 'previous': {
            'tagName': 'v1.2.2', 'isDraft': False, 'isPrerelease': False,
            'assets': [{'name': name} for name in self.expected('v1.2.2')]}}))
        self.env = dict(os.environ, RELEASE_STATE=str(self.state), TAG=TAG,
                        PATH=str(self.tools) + os.pathsep + os.environ['PATH'])
        for platform in PLATFORMS:
            archive = self.assets / f'yupana-{TAG}-{platform}.tar.gz'
            archive.write_bytes(platform.encode())
            Path(str(archive) + '.sha256').write_text(
                hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
        (self.assets / 'SHA256SUMS').write_text(''.join(
            p.read_text() for p in sorted(self.assets.glob('*.sha256'))))

    def expected(self, tag=TAG):
        archives = [f'yupana-{tag}-{platform}.tar.gz' for platform in PLATFORMS]
        return set(archives + [name + '.sha256' for name in archives] + ['SHA256SUMS'])

    def snapshot(self):
        return json.loads(self.state.read_text())

    def run_publish(self, script=None):
        return subprocess.run(['bash', str(script or ROOT / 'scripts/publish-release-assets.sh'), TAG],
                              cwd=self.assets, env=self.env, text=True, capture_output=True)

    def assert_latest_complete(self):
        for event in self.snapshot()['events']:
            latest = event['latest']
            self.assertEqual({v['name'] for v in latest['assets']}, self.expected(latest['tagName']), event)

    def test_model_detects_legacy_early_publication(self):
        # Positive control: GitHub permits an empty public release. The model
        # must expose its incomplete latest state, not enforce our new policy.
        subprocess.run(['gh', 'release', 'create', TAG], env=self.env, check=True)
        self.assertNotEqual(self.snapshot()['events'][0]['latest']['assets'],
                            self.snapshot()['previous']['assets'])
        self.assertEqual(self.snapshot()['events'][0]['latest']['assets'], [])

    def test_draft_upload_then_publish_and_idempotent_rerun(self):
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_latest_complete()
        state = self.snapshot()
        self.assertEqual(state['events'][0]['kind'], 'create')
        self.assertEqual(state['events'][-1]['kind'], 'publish')
        self.assertFalse(state['release']['isDraft'])
        before = len(state['events'])
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.snapshot()['events']), before)

    def test_existing_release_plz_draft_keeps_its_prerelease_flag(self):
        state = self.snapshot()
        state['release'] = {'tagName': TAG, 'isDraft': True, 'isPrerelease': True, 'assets': []}
        self.state.write_text(json.dumps(state))
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.snapshot()['release']['isPrerelease'])
        self.assertFalse(self.snapshot()['release']['isDraft'])
        self.assertIn('--latest=false', self.snapshot()['events'][-1]['args'])
        self.assert_latest_complete()

    def test_failed_upload_and_false_success_do_not_publish(self):
        for mode in ('FAIL_UPLOAD', 'PARTIAL_SUCCESS', 'BAD_STATE', 'BAD_SIZE', 'BAD_METADATA'):
            with self.subTest(mode=mode):
                self.env[mode] = '1'
                result = self.run_publish()
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertTrue(self.snapshot()['release']['isDraft'])
                self.assert_latest_complete()
                self.env.pop(mode)

    def test_transient_partial_upload_retries_while_draft(self):
        self.env['FAIL_FIRST_UPLOAD'] = '1'
        result = self.run_publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.snapshot()['uploads'], 2)
        self.assert_latest_complete()

    def test_missing_and_corrupt_local_artifacts_make_no_release(self):
        sidecar = self.assets / f'yupana-{TAG}-{PLATFORMS[0]}.tar.gz.sha256'
        sidecar.write_text('0' * 64 + '  ' + sidecar.name.removesuffix('.sha256') + '\n')
        result = self.run_publish()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.snapshot()['events'], [])
        sidecar.unlink()
        result = self.run_publish()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.snapshot()['events'], [])

    def test_published_incomplete_release_is_not_mutated(self):
        subprocess.run(['gh', 'release', 'create', TAG], env=self.env, check=True)
        before = self.snapshot()
        result = self.run_publish()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.snapshot(), before)

    def test_production_configuration_and_workflow_use_the_gated_publisher(self):
        config = tomllib.loads((ROOT / 'release-plz.toml').read_text())
        effective = dict(config['workspace'])
        for package in config.get('package', []):
            if package['name'] == 'yupana':
                effective.update(package)
        self.assertTrue(effective['git_release_draft'])
        self.assertFalse(effective['git_release_latest'])
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        publisher = workflow.split('  publish:\n', 1)[1].split('  release-plz:\n', 1)[0]
        self.assertIn('scripts/publish-release-assets.sh', publisher)
        self.assertIn('group: yupana-release-publication', publisher)
        self.assertIn('cancel-in-progress: false', publisher)


if __name__ == '__main__':
    unittest.main()
