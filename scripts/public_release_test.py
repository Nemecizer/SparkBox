"""Release guards; only temporary files and mocked external commands."""
import importlib.util
import json
from pathlib import Path
import subprocess
import shutil
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release', Path(__file__).with_name('public-release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
IDENTITY = 'Developer ID Application: Example (EXAMPLE123)'
LISTING = f'  1) {"A"*40} "{IDENTITY}"\n'


class ReleaseTests(unittest.TestCase):
    def test_only_an_available_explicit_developer_id_is_accepted(self):
        self.assertEqual(release.select_identity(IDENTITY, LISTING), IDENTITY)
        for name in [None, '', 'Apple Development: Example (EXAMPLE123)', 'Apple Distribution: Example (EXAMPLE123)', IDENTITY+' changed']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                release.select_identity(name, LISTING)
        with self.assertRaises(ValueError): release.select_identity(IDENTITY, '')

    def test_path_remapping_preserves_flags_and_space_containing_home(self):
        with patch.object(release.Path, 'home', return_value=Path('/Users/Example Person')):
            original = {'RUSTFLAGS': '-C opt-level=2 --cfg "feature=example"', 'KEEP': 'unchanged'}
            env = release.private_path_safe_env(original)
            self.assertEqual(env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f'),
                             ['-C', 'opt-level=2', '--cfg', 'feature=example',
                              '--remap-path-prefix=/Users/Example Person=/build/home',
                              f'--remap-path-prefix={release.ROOT.resolve()}=/build/sparkbox'])
            self.assertNotIn('RUSTFLAGS', env)
            self.assertEqual(env['KEEP'], 'unchanged')
            self.assertIn('RUSTFLAGS', original)

    def test_encoded_flags_keep_cargo_precedence(self):
        for encoded in ['', '-C\x1fopt-level=2']:
            env = release.private_path_safe_env({'CARGO_ENCODED_RUSTFLAGS': encoded, 'RUSTFLAGS': 'ignored'})
            self.assertNotIn('ignored', env['CARGO_ENCODED_RUSTFLAGS'])
            self.assertEqual(env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f')[:-2], encoded.split('\x1f') if encoded else [])

    def test_matching_provenance_cannot_hide_private_build_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); meta = self.make_candidate(root)
            binary = root/'SparkBox.app/binary'; binary.write_bytes(b'/Users/Fixture Owner/.cargo/source.rs')
            meta['files'][0]['sha256'] = release.digest(binary)
            (root/'PROVENANCE.json').write_text(json.dumps(meta))
            with patch.object(release.Path, 'home', return_value=Path('/Users/Fixture Owner')):
                with self.assertRaisesRegex(ValueError, 'build-machine paths'):
                    release.checked_candidate(root)
                with patch.object(release, 'run') as run:
                    with self.assertRaisesRegex(ValueError, 'build-machine paths'):
                        release.notarize(root, root/'notarized', 'profile', True)
                    run.assert_not_called()
                    self.assertFalse((root/'notarized').exists())

    def test_no_submit_means_no_external_command_or_output(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(release, 'run') as run:
            out = Path(temp)/'out'
            with self.assertRaisesRegex(ValueError, 'Nothing submitted'):
                release.notarize(Path(temp)/'in', out, 'profile', False)
            run.assert_not_called(); self.assertFalse(out.exists())

    def test_existing_output_is_preserved_before_submission(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(release, 'run') as run:
            out = Path(temp); marker = out/'keep'; marker.write_text('old')
            with self.assertRaisesRegex(ValueError, 'new directory'):
                release.notarize(out/'in', out, 'profile', True)
            self.assertEqual(marker.read_text(), 'old'); run.assert_not_called()

    def make_candidate(self, root):
        app = root/'SparkBox.app'; app.mkdir(); file = app/'binary'; file.write_bytes(b'app')
        archive = root/'candidate.zip'; archive.write_bytes(b'archive')
        meta = {'source_commit': 'a'*40, 'archive': archive.name, 'archive_sha256': release.digest(archive),
                'files': [{'path': 'SparkBox.app/binary', 'sha256': release.digest(file)}]}
        (root/'PROVENANCE.json').write_text(json.dumps(meta)); return meta

    def test_changed_archive_app_or_added_file_refuses(self):
        for which in ['archive', 'app', 'extra']:
            with self.subTest(which=which), tempfile.TemporaryDirectory() as temp:
                root=Path(temp); self.make_candidate(root); release.checked_candidate(root)
                p={'archive':root/'candidate.zip','app':root/'SparkBox.app/binary','extra':root/'SparkBox.app/extra'}[which]
                p.write_bytes(b'changed')
                with self.assertRaises(ValueError): release.checked_candidate(root)

    def test_provenance_cannot_escape_candidate(self):
        with tempfile.TemporaryDirectory() as temp:
            for name in ['../outside', '/etc/passwd']:
                with self.assertRaises(ValueError): release.safe_child(Path(temp), name)

    def test_unaccepted_submission_never_staples_or_produces_app(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); candidate=root/'in'; candidate.mkdir(); self.make_candidate(candidate)
            with patch.object(release, 'check_signature'), patch.object(release, 'run', return_value=subprocess.CompletedProcess([], 0, '{"status":"Invalid"}', '')) as run:
                with self.assertRaisesRegex(ValueError, 'did not accept'):
                    release.notarize(candidate, root/'out', 'profile', True)
                self.assertEqual(run.call_count,1)
                self.assertFalse((root/'out/SparkBox.app').exists())
                self.assertTrue((root/'out/notary-response.json').exists())

    def test_signature_must_have_developer_id_runtime_and_timestamp(self):
        good=f'Authority={IDENTITY}\nflags=0x10000(runtime)\nTimestamp=Today\n'
        for details in [good.replace(IDENTITY,'Apple Development: Example'), good.replace('(runtime)',''), good.replace('Timestamp=','Signed Time=')]:
            def run(args): return subprocess.CompletedProcess(args,0,'',details)
            with patch.object(release,'run',side_effect=run), self.assertRaises(ValueError):
                release.check_signature(Path('/fake.app'))

    def test_submission_error_retains_diagnostics_without_stapling(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); candidate=root/'in'; candidate.mkdir(); self.make_candidate(candidate)
            failure=subprocess.CalledProcessError(1,['notarytool'],output='not accepted',stderr='details')
            with patch.object(release,'check_signature'), patch.object(release,'run',side_effect=failure) as run:
                with self.assertRaisesRegex(ValueError,'diagnostics retained'):
                    release.notarize(candidate,root/'out','profile',True)
                self.assertEqual(run.call_count,1)
                self.assertEqual((root/'out/notary-error.txt').read_text(),'not accepteddetails')
                self.assertFalse((root/'out/SparkBox.app').exists())

    def test_accepted_workflow_preserves_input_and_checks_extracted_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); candidate=root/'in'; candidate.mkdir(); self.make_candidate(candidate)
            output=root/'out'; calls=[]
            def fake(args):
                calls.append(args)
                if args[:3]==['xcrun','notarytool','submit']:
                    return subprocess.CompletedProcess(args,0,'{"status":"Accepted","id":"test-only"}','')
                if args[0]=='ditto':
                    if '-c' in args:Path(args[-1]).write_bytes(b'test archive')
                    elif '-x' in args:shutil.copytree(output/'SparkBox.app',Path(args[-1])/'SparkBox.app')
                    else:shutil.copytree(args[1],args[2])
                if args[:3]==['xcrun','stapler','staple']:(Path(args[-1])/'ticket').write_text('test ticket')
                return subprocess.CompletedProcess(args,0,'','accepted')
            with patch.object(release,'check_signature') as verify, patch.object(release,'run',side_effect=fake):
                release.notarize(candidate,output,'profile',True)
                self.assertEqual(verify.call_count,3)
            self.assertFalse((candidate/'SparkBox.app/ticket').exists())
            result=json.loads((output/'PROVENANCE.json').read_text())
            self.assertTrue(result['ticket_stapled_and_validated'] and result['archive_verified'])
            self.assertFalse(result['published'] or result['launched'])
            self.assertEqual(result['archive_sha256'],release.digest(output/result['archive']))
            self.assertEqual(sum(a[:3]==['xcrun','stapler','validate'] for a in calls),2)

    def test_debug_entitlement_refuses(self):
        good=f'Authority={IDENTITY}\nflags=0x10000(runtime)\nTimestamp=Today\n'
        def run(args):
            out=release.plistlib.dumps({'com.apple.security.get-task-allow':True}).decode() if '--entitlements' in args else ''
            return subprocess.CompletedProcess(args,0,out,good)
        with patch.object(release,'run',side_effect=run), self.assertRaisesRegex(ValueError,'Debug'):
            release.check_signature(Path('/fake.app'))

    def test_build_disables_implicit_submission(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); (root/'LICENSE').write_text('test')
            with patch.object(release,'ROOT',root), patch.object(release,'preflight',return_value=IDENTITY), patch.dict(release.os.environ,{k:'secret-test-only' for k in release.NOTARY_ENV}), patch.object(release.subprocess,'run') as run:
                release.build('candidate')
                env=run.call_args.kwargs['env']
                self.assertTrue(all(k not in env for k in release.NOTARY_ENV))
                self.assertEqual(env['APPLE_SIGNING_IDENTITY'],IDENTITY)
                self.assertIn(f'--remap-path-prefix={Path.home()}=/build/home', env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f'))
                self.assertTrue(env['CARGO_ENCODED_RUSTFLAGS'].endswith(f'--remap-path-prefix={root.resolve()}=/build/sparkbox'))


if __name__ == '__main__': unittest.main()
