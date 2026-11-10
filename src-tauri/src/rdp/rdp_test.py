import hashlib
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

HERE = Path(__file__).parent
source = (HERE / 'rdp.py').read_text().replace('__XFCE_LAUNCHER__', repr((HERE / 'xfce.sh').read_text())).replace('__GNOME_LAUNCHER__', repr((HERE / 'gnome.sh').read_text()))
rdp = types.ModuleType('rdp_under_test')
exec(compile(source, str(HERE / 'rdp.py'), 'exec'), rdp.__dict__)

INI = '[Globals]\nport=3390\nsecurity_layer=negotiate\ncertificate=/etc/xrdp/spark-rdp.crt\nkey_file=/etc/xrdp/spark-rdp.key\n\n[Xorg]\nname=Xorg\n'
SESMAN = '[Globals]\nEnableUserWindowManager=true\nDefaultWindowManager=startwm.sh\n[Xorg]\nparam=Xorg\nparam=-config\nparam=xrdp/xorg.conf\n'
XORG = 'Section "ServerFlags"\nEndSection\nSection "Device"\n Option "DRMDevice" "/dev/dri/renderD128"\nEndSection\nSection "Screen"\nEndSection\nSection "Module"\n Load "xorgxrdp"\nEndSection\n'


class Parsing(unittest.TestCase):
    def test_ini_keeps_repeated_xorg_parameters_and_other_sections(self):
        out = rdp.edit_ini(SESMAN, 'Globals', {'DefaultWindowManager': '/etc/xrdp/spark-startwm.sh'})
        self.assertEqual(out.split('[Xorg]')[1], SESMAN.split('[Xorg]')[1])
        self.assertIn('EnableUserWindowManager=true', out)
        self.assertEqual(rdp.edit_ini(out, 'Globals', {'DefaultWindowManager': '/etc/xrdp/spark-startwm.sh'}), out)

    def test_ambiguous_ini_is_refused(self):
        for text in ['[other]\nx=y\n', SESMAN + '[Globals]\nx=y\n', SESMAN.replace('EnableUserWindowManager=true', 'EnableUserWindowManager=true\nEnableUserWindowManager=false')]:
            with self.assertRaises(RuntimeError):
                rdp.edit_ini(text, 'Globals', {'EnableUserWindowManager': 'false'})

    def test_xorg_scopes_changes_and_is_idempotent(self):
        out = rdp.edit_xorg(XORG)
        self.assertIn('"DRMDevice" "/dev/dri/xrdp-software-only"', out)
        self.assertIn('"AutoAddGPU" "off"', out)
        self.assertIn('GPUDevice ""', out)
        self.assertLess(out.index('Load "glamoregl"'), out.index('Load "xorgxrdp"'))
        self.assertEqual(rdp.edit_xorg(out), out)
        with self.assertRaises(RuntimeError):
            rdp.edit_xorg(XORG + 'Section "Device"\nEndSection\n')

    def test_negotiation_handles_fragmentation_and_rejects_non_tls(self):
        class Sock:
            def __init__(self, data): self.data = data; self.sent = b''
            def sendall(self, data): self.sent += data
            def recv(self, _):
                data, self.data = self.data[:1], self.data[1:]
                return data
        good = bytes.fromhex('030000130ed000000000000200080001000000')
        sock = Sock(good); rdp.negotiation(sock)
        self.assertEqual(len(sock.sent), 19)
        rdp.negotiation(Sock(good[:12] + b"\x01" + good[13:]))
        for bad in [b'', b'HTTP', good[:-4] + b'\0\0\0\0', good[:11] + b'\3' + good[12:]]:
            with self.assertRaises(RuntimeError): rdp.negotiation(Sock(bad))

    def test_launchers_match_supplied_desktop_contract(self):
        self.assertIn('LIBGL_ALWAYS_SOFTWARE=1', rdp.GNOME)
        self.assertIn('XAUTHORITY="$HOME/.Xauthority"', rdp.GNOME)
        self.assertIn('[ ! -S "$XDG_RUNTIME_DIR/bus" ]', rdp.GNOME)
        self.assertIn('dbus-update-activation-environment --systemd', rdp.GNOME)
        self.assertIn('exec dbus-run-session -- startxfce4', rdp.XFCE)


class NodeFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.root = Path(self.temp.name)
        def path(p):
            p = Path(p)
            return p if str(p).startswith(str(self.root)) else self.root / str(p).lstrip('/')
        self.path = path
        self.active_sessions = []; self.fail_start = False; self.changed = False; self.calls = []
        self.states = {u: {'LoadState': 'loaded', 'ActiveState': 'active', 'UnitFileState': 'enabled'} for u in rdp.UNITS}
        for p, text in dict(zip(rdp.CONFIGS, [INI, SESMAN, XORG, rdp.XFCE]), **{
            '/etc/os-release': 'ID=ubuntu\nVERSION_ID="24.04"\n', '/proc/sys/kernel/random/boot_id': 'boot-one',
            rdp.CERT: 'public cert', rdp.KEY: 'fixture key', '/usr/bin/startxfce4': '', '/usr/bin/dbus-run-session': '',
            '/usr/bin/gnome-session': '', '/usr/bin/gnome-shell': '', '/usr/bin/dbus-update-activation-environment': '',
            '/usr/share/gnome-session/sessions/ubuntu.session': '',
        }).items():
            dest = path(p); dest.parent.mkdir(parents=True, exist_ok=True); dest.write_text(text); dest.chmod(0o755 if p == rdp.CONFIGS[3] else 0o644)
        path('/var/backups').mkdir(parents=True); path('/run/lock').mkdir(parents=True)
        def state(p):
            f = path(p)
            if not f.exists(): return None
            st = f.stat()
            return [rdp.sha(f.read_bytes()), st.st_mode & 0o777, 0, st.st_gid]
        def run(*args, **_):
            self.calls.append(args)
            if args[:2] == ('dpkg', '--print-architecture'): return 'arm64'
            if args[0] == 'dpkg-query': return '\n'.join(n + ' installed' for n in ['xrdp', 'xorgxrdp', 'libgl1-mesa-dri'])
            if args[:2] == ('ss', '-H'): return 'LISTEN 0 128 *:3390 *:* users:(("xrdp",pid=10,fd=3))'
            if args[:2] == ('passwd', '-S'): return 'owner P 2026-01-01 0 99999 7 -1'
            if args[0] == 'ufw': return 'Status: active'
            if args[:2] == ('systemctl', 'start') and self.fail_start:
                self.fail_start = False
                raise RuntimeError('fixture service failure')
            return ''
        self.patchers = [patch.object(rdp, 'Path', side_effect=path), patch.object(rdp, 'safe_parent'), patch.object(rdp.os, 'fstat', return_value=types.SimpleNamespace(st_uid=0)), patch.object(rdp, 'file_state', side_effect=state),
            patch.object(rdp, 'read', side_effect=lambda p, missing=False: path(p).read_text() if path(p).exists() else ''),
            patch.object(rdp, 'run', side_effect=run), patch.object(rdp, 'service', side_effect=lambda u: dict(self.states[u])),
            patch.object(rdp, 'sessions', side_effect=lambda: list(self.active_sessions)),
            patch.object(rdp, 'cert_info', return_value='a1' * 32), patch.object(rdp, 'probe', return_value='TLSv1.3'),
            patch.object(rdp.pwd, 'getpwnam', return_value=types.SimpleNamespace(pw_uid=1000)),
            patch.dict(os.environ, {'SUDO_USER': 'owner'}), patch.object(rdp.os, 'fchown'), patch.object(rdp.shutil, 'which', return_value='/usr/sbin/ufw')]
        for p in self.patchers: p.start()

    def tearDown(self):
        for p in reversed(self.patchers): p.stop()
        self.temp.cleanup()

    def test_keep_desktop_review_is_nonmutating_and_stable(self):
        before = [self.path(p).read_bytes() for p in rdp.CONFIGS]
        report, plan = rdp.inspect('keep')
        self.assertFalse(report['blockers'], report)
        self.assertEqual(report['profile'], 'xfce')
        self.assertEqual(report['port'], 3390)
        self.assertEqual(rdp.inspect('keep')[0]['digest'], report['digest'])
        self.assertEqual(before, [self.path(p).read_bytes() for p in rdp.CONFIGS])
        self.assertNotIn('fixture key', json.dumps(report))
        self.assertFalse(any(c[:2] in [('systemctl', 'stop'), ('systemctl', 'start')] for c in self.calls))

    def test_explicit_gnome_profile_preserves_settings_and_parameters(self):
        report, plan = rdp.inspect('gnome')
        self.assertEqual(report['profile'], 'gnome')
        self.assertEqual(plan['after'][rdp.CONFIGS[3]], rdp.GNOME)
        self.assertEqual(plan['after'][rdp.CONFIGS[1]].split('[Xorg]')[1], SESMAN.split('[Xorg]')[1])

    def test_active_or_disconnected_sessions_block_repair_before_writes(self):
        report = rdp.inspect('keep')[0]; self.active_sessions = ['42']
        with self.assertRaisesRegex(RuntimeError, 'Log out'):
            rdp.repair('keep', report['digest'], 'dev')
        self.assertFalse(any(c[:2] == ('systemctl', 'stop') for c in self.calls))

    def test_changed_review_refuses_and_preserves_concurrent_edit(self):
        report = rdp.inspect('keep')[0]
        self.path(rdp.CONFIGS[0]).write_text(INI + '# owner edit\n')
        with self.assertRaisesRegex(RuntimeError, 'review changed'):
            rdp.repair('keep', report['digest'], 'dev')
        self.assertIn('# owner edit', self.path(rdp.CONFIGS[0]).read_text())

    def test_boot_change_refuses_without_stopping_services(self):
        report = rdp.inspect('keep')[0]
        self.path('/proc/sys/kernel/random/boot_id').write_text('boot-two')
        with self.assertRaisesRegex(RuntimeError, 'review changed'):
            rdp.repair('keep', report['digest'], 'dev')

    def test_success_keeps_private_backup_and_verifies_encrypted_server(self):
        report = rdp.inspect('keep')[0]
        after = rdp.repair('keep', report['digest'], 'dev')
        self.assertFalse(after['changes'], after)
        backup = Path(after['backup'])
        self.assertEqual(backup.stat().st_mode & 0o777, 0o700)
        self.assertEqual((backup / 'xrdp.ini').read_text(), INI)
        self.assertTrue(rdp.probe.called)
        self.assertFalse(any(c[0] in ('apt-get', 'reboot') or c[:2] in [('ufw', 'allow'), ('passwd', 'owner')] for c in self.calls))

    def test_failed_start_rolls_back_exact_files(self):
        report = rdp.inspect('keep')[0]
        before = [self.path(p).read_bytes() for p in rdp.CONFIGS]
        self.fail_start = True
        with self.assertRaisesRegex(RuntimeError, 'Previous configuration and service state restored'):
            rdp.repair('keep', report['digest'], 'dev')
        self.assertEqual(before, [self.path(p).read_bytes() for p in rdp.CONFIGS])

    def test_mask_and_missing_desktop_fail_closed(self):
        self.states['xrdp.service']['UnitFileState'] = 'masked'
        self.assertIn('masked', ' '.join(rdp.inspect('keep')[0]['blockers']))
        self.path('/usr/bin/gnome-shell').unlink()
        self.assertIn('not installed', ' '.join(rdp.inspect('gnome')[0]['blockers']))

    def test_failed_verification_does_not_overwrite_a_later_owner_edit(self):
        report = rdp.inspect('keep')[0]
        calls = []
        def changed_probe(*_):
            calls.append(1)
            if len(calls) == 1: return 'TLSv1.3'
            self.path(rdp.CONFIGS[0]).write_text('# owner changed it during repair\n')
            raise RuntimeError('fixture TLS failure')
        with patch.object(rdp, 'probe', side_effect=changed_probe), patch.object(rdp.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'Rollback incomplete'):
                rdp.repair('keep', report['digest'], 'dev')
        self.assertEqual(self.path(rdp.CONFIGS[0]).read_text(), '# owner changed it during repair\n')

    def test_port_conflict_uses_a_free_port_without_stopping_the_other_server(self):
        original = rdp.run.side_effect
        def conflict(*args, **kwargs):
            if args[0] == 'ss': return 'LISTEN 0 128 *:3390 *:* users:(("gnome-remote-desktop",pid=11,fd=3))\nLISTEN 0 128 *:3389 *:* users:(("other",pid=12,fd=3))'
            return original(*args, **kwargs)
        with patch.object(rdp, 'run', side_effect=conflict):
            report, plan = rdp.inspect('keep')
        self.assertEqual(report['port'], 3391)
        self.assertIn('port=3391', plan['after'][rdp.CONFIGS[0]])
        self.assertFalse(any('gnome-remote-desktop' in c for c in self.calls))

    def test_new_certificate_failure_restores_without_exposing_private_material(self):
        with patch.object(rdp, 'cert_info', side_effect=RuntimeError('expired')):
            report = rdp.inspect('keep')[0]
            self.assertTrue(any('certificate identity will change' in c for c in report['changes']))
            with self.assertRaisesRegex(RuntimeError, 'Previous configuration and service state restored'):
                rdp.repair('keep', report['digest'], 'dev')
        self.assertEqual(self.path(rdp.KEY).read_text(), 'fixture key')
        self.assertNotIn('fixture key', json.dumps(report))

    def test_new_certificate_is_private_and_its_previous_identity_is_backed_up(self):
        original = rdp.run.side_effect
        def generate(*args, **kwargs):
            if args[:2] == ('openssl', 'req'):
                Path(args[args.index('-keyout') + 1]).write_text('replacement private key')
                Path(args[args.index('-out') + 1]).write_text('replacement certificate')
                return ''
            return original(*args, **kwargs)
        def certificate(*_):
            if self.path(rdp.KEY).read_text() != 'replacement private key':
                raise RuntimeError('The previous certificate expired')
            return 'b2' * 32
        with patch.object(rdp, 'run', side_effect=generate), patch.object(rdp, 'cert_info', side_effect=certificate), patch('grp.getgrnam', return_value=types.SimpleNamespace(gr_gid=100)):
            report = rdp.inspect('keep')[0]
            after = rdp.repair('keep', report['digest'], 'dev')
        self.assertFalse(after['changes'], after)
        self.assertEqual(after['fingerprint'], 'b2' * 32)
        self.assertEqual(self.path(rdp.KEY).stat().st_mode & 0o777, 0o640)
        self.assertEqual((Path(after['backup']) / 'spark-rdp.key').read_text(), 'fixture key')
        self.assertNotIn('replacement private key', json.dumps(after))


if __name__ == '__main__': unittest.main()
