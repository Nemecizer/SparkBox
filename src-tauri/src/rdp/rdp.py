"""Bounded xrdp inspection and configuration repair. No credentials leave the node.

The application embeds this source and the owner's two desktop launchers. It runs
through the authenticated sudo exit, never from a downloaded or writable helper.
Package installation, firewall changes and Linux password changes are deliberately
separate owner actions. A listening socket does not prove a desktop login.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import signal
import socket
import ssl
import stat
import struct
import subprocess
import sys
import tempfile
import time

XFCE = __XFCE_LAUNCHER__
GNOME = __GNOME_LAUNCHER__
CONFIGS = ['/etc/xrdp/xrdp.ini', '/etc/xrdp/sesman.ini',
           '/etc/X11/xrdp/xorg.conf', '/etc/xrdp/spark-startwm.sh']
CERT = '/etc/xrdp/spark-rdp.crt'
KEY = '/etc/xrdp/spark-rdp.key'
UNITS = ['xrdp-sesman.service', 'xrdp.service']
ENV = dict(os.environ, PATH='/usr/sbin:/usr/bin:/sbin:/bin', LC_ALL='C', SYSTEMD_PAGER='', PAGER='cat')


def run(*args, codes=(0,), timeout=15):
    p = subprocess.run(args, capture_output=True, text=True, timeout=timeout, env=ENV)
    if p.returncode not in codes:
        raise RuntimeError(f'{args[0]} failed ({p.returncode}): {p.stderr.strip()[:500]}')
    return p.stdout.strip()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def safe_parent(path):
    for parent in Path(path).parents:
        st = parent.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o022:
            raise RuntimeError(f'Unsafe configuration directory: {parent}')


def read(path, missing=False):
    safe_parent(path)
    p = Path(path)
    if not p.exists() and not p.is_symlink() and missing:
        return ''
    # Never follow a writable or unexpected configuration link as root.
    st = p.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o022 or st.st_size > 131072:
        raise RuntimeError(f'Unsafe or unsupported configuration file: {path}')
    return p.read_text()


def ini(text, section):
    pattern = r'(?ms)^\[' + re.escape(section) + r'\][^\n]*\n(.*?)(?=^\[|\Z)'
    matches = list(re.finditer(pattern, text))
    if len(matches) != 1:
        raise RuntimeError(f'Expected exactly one [{section}] section')
    return matches[0]


def value(text, section, key, default=''):
    values = re.findall(r'(?im)^[ \t]*' + re.escape(key) + r'[ \t]*=[ \t]*([^\n]*)', ini(text, section)[1])
    if len(values) > 1:
        raise RuntimeError(f'Duplicate {key}; review the configuration')
    return values[0].strip() if values else default


def edit_ini(text, section, settings):
    m = ini(text, section)
    body = m[1]
    if body and not body.endswith('\n'):
        body += '\n'
    for key, val in settings.items():
        value(text, section, key)  # fail closed on duplicates
        pattern = r'(?im)^[ \t]*' + re.escape(key) + r'[ \t]*=[^\n]*'
        line = key + '=' + val
        body, count = re.subn(pattern, lambda _: line, body)
        if not count:
            body += line + '\n'
    return text[:m.start(1)] + body + text[m.end(1):]


def edit_xorg(text):
    modules = re.findall(r'(?im)^[ \t]*Load\s+"xorgxrdp"[^\n]*', text)
    if len(modules) != 1:
        raise RuntimeError('Expected one xorgxrdp module')
    # Edit inside the intended single section, never globally across other GPUs.
    for section, pattern, replacement in [
        ('Device', r'Option\s+"DRMDevice"[^\n]*', 'Option "DRMDevice" "/dev/dri/xrdp-software-only"'),
        ('ServerFlags', r'Option\s+"AutoAddGPU"[^\n]*', 'Option "AutoAddGPU" "off"'),
        ('Screen', r'GPUDevice[^\n]*', 'GPUDevice ""'),
    ]:
        matches = list(re.finditer(r'(?ims)^[ \t]*Section\s+"' + section + r'"[^\n]*\n(.*?)^[ \t]*EndSection\b', text))
        if len(matches) != 1:
            raise RuntimeError(f'Expected one Xorg {section} section')
        m = matches[0]
        body, count = re.subn(r'(?im)^[ \t]*' + pattern, lambda _: '    ' + replacement, m[1])
        if count > 1:
            raise RuntimeError(f'Duplicate Xorg setting in {section}')
        if not count:
            body = '    ' + replacement + '\n' + body
        text = text[:m.start(1)] + body + text[m.end(1):]
    if not re.search(r'(?im)^\s*Load\s+"glamoregl"', text):
        text, count = re.subn(r'(?im)^([ \t]*Load\s+"xorgxrdp"[^\n]*)', lambda m: '    Load "glamoregl"\n' + m[1], text)
        if count != 1:
            raise RuntimeError('Expected one xorgxrdp module')
    return text


def file_state(path):
    p = Path(path)
    if not p.exists() and not p.is_symlink():
        return None
    st = p.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_uid != 0 or st.st_size > 131072:
        raise RuntimeError(f'Unsafe repair target: {path}')
    return [sha(p.read_bytes()), stat.S_IMODE(st.st_mode), st.st_uid, st.st_gid]


def cert_info(cert, key):
    if not cert.startswith('/etc/') or not key.startswith('/etc/'):
        raise RuntimeError('Certificate paths must be under /etc')
    for filename, forbidden in [(cert, 0o022), (key, 0o027)]:
        st = Path(filename).stat()
        if not stat.S_ISREG(st.st_mode) or st.st_uid != 0 or st.st_mode & forbidden:
            raise RuntimeError('The certificate or private key has unsafe ownership or permissions')
    # Public certificate only; never print or return a private key.
    run('openssl', 'x509', '-in', cert, '-noout', '-checkend', '86400')
    start = run('openssl', 'x509', '-in', cert, '-noout', '-startdate')
    if not start.startswith('notBefore=') or ssl.cert_time_to_seconds(start[10:]) > time.time():
        raise RuntimeError('The certificate is not yet valid')
    der = ssl.PEM_cert_to_DER_cert(Path(cert).read_text())
    fingerprint = sha(der)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)  # checks matching key without returning it
    run('runuser', '-u', 'xrdp', '--', 'test', '-r', key)
    return fingerprint


def negotiation(sock):
    sock.sendall(bytes.fromhex('030000130ee000000000000100080001000000'))
    def exact(n):
        out = b''
        while len(out) < n:
            b = sock.recv(n - len(out))
            if not b:
                raise RuntimeError('RDP negotiation ended early')
            out += b
        return out
    header = exact(4)
    length = struct.unpack('!H', header[2:])[0]
    if header[:2] != b'\x03\x00' or length != 19:
        raise RuntimeError('Unexpected RDP negotiation reply')
    reply = exact(length - 4)
    if reply[0] != 14 or reply[1] != 0xd0 or reply[-8] != 2 or reply[-6:] != bytes.fromhex('080001000000'):
        raise RuntimeError('Server did not accept RDP with TLS')


def probe(port, fingerprint):
    # Cert is authenticated separately over the already trusted SSH session.
    # TLS still proves possession of that exact private key; no login is sent.
    with socket.create_connection(('127.0.0.1', port), timeout=4) as raw:
        raw.settimeout(4)
        negotiation(raw)
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        ctx.minimum_version = ssl.TLSVersion.TLSv1_2
        with ctx.wrap_socket(raw) as tls:
            if sha(tls.getpeercert(binary_form=True)) != fingerprint:
                raise RuntimeError('RDP certificate differs from the configured certificate')
            return tls.version()


def service(unit):
    lines = run('systemctl', 'show', unit, '--property=LoadState,ActiveState,UnitFileState').splitlines()
    result = dict(x.split('=', 1) for x in lines if '=' in x)
    if not all(k in result for k in ['LoadState', 'ActiveState', 'UnitFileState']):
        raise RuntimeError('Incomplete service state')
    return result


def sessions():
    # Disconnected desktops retain chansrv too; do not destroy their work.
    return run('pgrep', '-x', 'xrdp-chansrv', codes=(0, 1)).splitlines()


def inspect(profile):
    result = dict(port=None, user=os.environ.get('SUDO_USER', ''), profile=profile,
                  fingerprint=None, checks=[], changes=[], blockers=[], digest='', backup=None)
    checks, changes, blockers = result['checks'], result['changes'], result['blockers']
    def check(label, ok, detail):
        checks.append(dict(label=label, ok=ok, detail=detail))
    files, after = {}, {}
    try:
        os_release = Path('/etc/os-release').read_text()
        if not re.search(r'(?m)^ID=["\']?ubuntu["\']?$', os_release) or not re.search(r'(?m)^VERSION_ID=["\']?24\.04["\']?$', os_release):
            raise RuntimeError('Repair supports Ubuntu 24.04 only')
        if run('dpkg', '--print-architecture') != 'arm64':
            raise RuntimeError('Repair supports Spark arm64 nodes only')
        user = pwd.getpwnam(result['user'])
        if user.pw_uid == 0:
            raise RuntimeError('Use the normal Linux login account, not root')
        packages = run('dpkg-query', '-W', '-f=${Package} ${db:Status-Status}\n', 'xrdp', 'xorgxrdp', 'libgl1-mesa-dri', codes=(0, 1))
        missing = [n for n in ['xrdp', 'xorgxrdp', 'libgl1-mesa-dri'] if n + ' installed' not in packages.splitlines()]
        if missing:
            raise RuntimeError('Install these prerequisites through your normal package process, then check again: ' + ', '.join(missing))
        check('Installed RDP software', True, 'xrdp, xorgxrdp and software rendering are installed')
        for path in CONFIGS:
            files[path] = read(path, missing=path == CONFIGS[3])
        current = files[CONFIGS[3]]
        detected = 'gnome' if 'exec /usr/bin/gnome-session --session=ubuntu' in current else 'xfce' if 'exec dbus-run-session -- startxfce4' in current else None
        chosen = detected if profile == 'keep' else profile
        result['profile'] = chosen or 'unknown'
        if chosen not in ('gnome', 'xfce'):
            raise RuntimeError('Choose Ubuntu GNOME or XFCE before replacing an unrecognized desktop launcher')
        needed = ['/usr/bin/gnome-session', '/usr/bin/gnome-shell', '/usr/bin/dbus-update-activation-environment', '/usr/share/gnome-session/sessions/ubuntu.session'] if chosen == 'gnome' else ['/usr/bin/startxfce4', '/usr/bin/dbus-run-session']
        if any(not Path(p).is_file() for p in needed):
            raise RuntimeError('The selected desktop is not installed; install it through your normal package process first')
        states = {u: service(u) for u in UNITS}
        if states['xrdp.service']['UnitFileState'] not in ('enabled', 'disabled'):
            blockers.append('Review the custom or masked xrdp startup setting before repair')
        for u, s in states.items():
            if s['LoadState'] != 'loaded' or s['UnitFileState'] == 'masked':
                blockers.append(u + ' is missing or masked; review that deliberate setting first')
            check(u, s['ActiveState'] == 'active', s['ActiveState'])
        active = sessions()
        check('Existing desktop sessions', None if active else True, f'{len(active)} desktop session process(es); log out before repair')
        if active:
            blockers.append('Log out of every RDP desktop before repair; closing its window is not enough')
        port_text = value(files[CONFIGS[0]], 'Globals', 'port', '3389')
        if not re.fullmatch(r'\d{4}', port_text) or not 3389 <= int(port_text) <= 3399:
            raise RuntimeError('Custom RDP listener; repair supports a single port from 3389 to 3399')
        port = int(port_text)
        listeners = run('ss', '-H', '-ltnp')
        def on_port(p):
            return [line for line in listeners.splitlines() if len(line.split()) >= 4 and line.split()[3].rsplit(':', 1)[-1] == str(p)]
        occupied = on_port(port)
        if any('"xrdp"' not in line for line in occupied):
            port = next((p for p in range(3389, 3400) if not on_port(p)), None)
            if port is None:
                raise RuntimeError('No free RDP port from 3389 to 3399; other services are left alone')
        result['port'] = port
        cert = value(files[CONFIGS[0]], 'Globals', 'certificate', CERT) or CERT
        key = value(files[CONFIGS[0]], 'Globals', 'key_file', KEY) or KEY
        generate = False
        try:
            result['fingerprint'] = cert_info(cert, key)
        except (RuntimeError, OSError, ValueError, ssl.SSLError):
            cert, key, generate = CERT, KEY, True
            changes.append('Create a private xrdp TLS certificate and key (the RDP certificate identity will change)')
        check('RDP certificate and private-key access', not generate, 'Certificate valid for at least one day, matching key readable by xrdp' if not generate else 'Repair will create a dedicated certificate')
        after[CONFIGS[0]] = edit_ini(files[CONFIGS[0]], 'Globals', dict(port=str(port), security_layer='tls', certificate=cert, key_file=key, ssl_protocols='TLSv1.2, TLSv1.3'))
        after[CONFIGS[1]] = edit_ini(files[CONFIGS[1]], 'Globals', dict(EnableUserWindowManager='false', DefaultWindowManager=CONFIGS[3]))
        after[CONFIGS[2]] = edit_xorg(files[CONFIGS[2]])
        after[CONFIGS[3]] = GNOME if chosen == 'gnome' else XFCE
        for path in CONFIGS:
            if after[path] != files[path] or (path == CONFIGS[3] and file_state(path)[1] != 0o755):
                changes.append('Configure ' + path)
        if states['xrdp.service']['UnitFileState'] != 'enabled':
            changes.append('Enable xrdp at startup')
        if changes or any(s['ActiveState'] != 'active' for s in states.values()):
            changes.append('Restart xrdp and xrdp-sesman; preserve GNOME Remote Desktop and other services')
        check('Desktop and NVIDIA software rendering', all(after[p] == files[p] for p in CONFIGS), 'Selected desktop: ' + chosen)
        password = run('passwd', '-S', result['user']).split()
        check('Linux password', len(password) > 1 and password[1] == 'P', 'A usable Linux password is required; SparkBox never changes it')
        if len(password) < 2 or password[1] != 'P':
            blockers.append('Set your Linux password yourself before repair or RDP login')
        ufw = run('ufw', 'status', codes=(0, 1)) if shutil.which('ufw') else 'UFW is not installed'
        check('Firewall', None, ufw[:2000] + '\nNo firewall rules are changed by repair')
        try:
            if not result['fingerprint']:
                raise RuntimeError('No usable certificate')
            tls = probe(int(port_text), result['fingerprint'])
            check('RDP negotiation on the node', True, tls + '; no desktop login attempted')
        except Exception as e:
            check('RDP negotiation on the node', False, str(e)[:300])
            if not any(c.startswith('Restart ') for c in changes):
                changes.append('Restart xrdp and xrdp-sesman, then verify encrypted RDP again')
        evidence = dict(files={p: file_state(p) for p in CONFIGS + [CERT, KEY]}, states=states,
                        port=port, profile=chosen, generate=generate, fingerprint=result['fingerprint'],
                        changes=changes, user=result['user'], boot=Path('/proc/sys/kernel/random/boot_id').read_text().strip())
        result['digest'] = sha(json.dumps(evidence, sort_keys=True).encode())
        return result, dict(files=files, after=after, states=states, generate=generate, evidence=evidence)
    except Exception as e:
        blockers.append(str(e)[:500])
        check('Configuration inspection', None, str(e)[:500])
        return result, None


def atomic(path, data, mode, uid=0, gid=0):
    safe_parent(path)
    p = Path(path)
    fd, temp = tempfile.mkstemp(prefix='.sparkbox-rdp-', dir=p.parent)
    try:
        with os.fdopen(fd, 'wb') as f:
            f.write(data)
            f.flush()
            os.fchmod(f.fileno(), mode)
            os.fchown(f.fileno(), uid, gid)
            os.fsync(f.fileno())
        os.replace(temp, p)
    finally:
        if os.path.exists(temp):
            os.unlink(temp)


def repair(profile, digest, instance):
    # root-owned lock also serializes repairs from other application instances.
    fd = os.open(str(Path('/run/lock/sparkbox-rdp.lock')), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as lock:
        if os.fstat(lock.fileno()).st_uid != 0:
            raise RuntimeError('The RDP transaction lock is not owned by root')
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        report, plan = inspect(profile)
        if not plan or report['blockers'] or report['digest'] != digest:
            raise RuntimeError('Repair refused: the review changed or a prerequisite is blocked. Check again. ' + '; '.join(report['blockers']))
        if not report['changes']:
            return report
        backup = Path(tempfile.mkdtemp(prefix='sparkbox-rdp-', dir=str(Path('/var/backups'))))
        os.chmod(backup, 0o700)
        originals, written = {}, {}
        for p in CONFIGS + ([CERT, KEY] if plan['generate'] else []):
            safe_parent(p)
            old = file_state(p)
            originals[p] = (Path(p).read_bytes(), old) if old else (None, None)
            if old:
                shutil.copy2(Path(p), backup / Path(p).name)
        (backup / 'manifest.json').write_text(json.dumps(dict(instance=instance, states=plan['states'], files={p: v[1] for p, v in originals.items()}), indent=2))
        try:
            if sessions():
                raise RuntimeError('A desktop session appeared; repair refused')
            # Stop listener first to prevent a new login during the transaction.
            run('systemctl', 'stop', 'xrdp.service', timeout=30)
            if sessions():
                raise RuntimeError('A desktop session appeared; repair refused')
            run('systemctl', 'stop', 'xrdp-sesman.service', timeout=30)
            if plan['generate']:
                temp_cert, temp_key = backup / 'new.crt', backup / 'new.key'
                run('openssl', 'req', '-x509', '-newkey', 'rsa:3072', '-sha256', '-nodes', '-days', '825', '-subj', '/CN=SparkBox-RDP', '-keyout', str(temp_key), '-out', str(temp_cert), timeout=30)
                import grp
                gid = grp.getgrnam('xrdp').gr_gid
                for p, data, mode in [(CERT, temp_cert.read_bytes(), 0o644), (KEY, temp_key.read_bytes(), 0o640)]:
                    if file_state(p) != plan['evidence']['files'][p]:
                        raise RuntimeError('Certificate changed during repair: ' + p)
                    atomic(p, data, mode, gid=gid)
                    written[p] = file_state(p)
            for p, text in plan['after'].items():
                if file_state(p) != plan['evidence']['files'][p]:
                    raise RuntimeError('Configuration changed during repair: ' + p)
                old = originals[p][1]
                atomic(p, text.encode(), 0o755 if p == CONFIGS[3] else old[1], gid=old[3] if old else 0)
                written[p] = file_state(p)
            run('sh', '-n', CONFIGS[3])
            run('systemctl', 'enable', 'xrdp.service')
            run('systemctl', 'start', *UNITS, timeout=30)
            last_error = ''
            for _ in range(10):
                try:
                    cert = value(plan['after'][CONFIGS[0]], 'Globals', 'certificate')
                    key = value(plan['after'][CONFIGS[0]], 'Globals', 'key_file')
                    fingerprint = cert_info(cert, key)
                    probe(report['port'], fingerprint)
                    if any(service(u)['ActiveState'] != 'active' for u in UNITS):
                        raise RuntimeError('RDP services are not active')
                    break
                except Exception as e:
                    last_error = str(e)
                    time.sleep(1)
            else:
                raise RuntimeError('RDP verification failed: ' + last_error)
            result, _ = inspect(profile)
            result['backup'] = str(backup)
            return result
        except BaseException as e:
            # Restore only files still byte-for-byte ours; concurrent edits win.
            problems = []
            can_restore_services = False
            try:
                if sessions():
                    problems.append('A new desktop session is present; its services were left running')
                else:
                    run('systemctl', 'stop', 'xrdp.service', 'xrdp-sesman.service', timeout=30)
                    can_restore_services = True
            except Exception as stop_error:
                problems.append('Could not stop RDP for recovery: ' + str(stop_error))
            for p, expected in reversed(list(written.items())):
                try:
                    if file_state(p) != expected:
                        raise RuntimeError('changed since repair')
                    data, old = originals[p]
                    if data is None:
                        Path(p).unlink()
                    else:
                        atomic(p, data, old[1], old[2], old[3])
                except Exception as restore_error:
                    problems.append(p + ': ' + str(restore_error))
            for u, s in plan['states'].items():
                try:
                    if not can_restore_services:
                        continue
                    if u == 'xrdp.service' and s['UnitFileState'] == 'disabled':
                        run('systemctl', 'disable', u)
                    run('systemctl', 'start' if s['ActiveState'] == 'active' else 'stop', u, timeout=30)
                except Exception as restore_error:
                    problems.append(str(restore_error))
            raise RuntimeError(f'Repair failed: {e}. Backup: {backup}. ' + ('Rollback incomplete: ' + '; '.join(problems) if problems else 'Previous configuration and service state restored.')) from e


def main():
    mode, profile, digest, instance = sys.argv[1:]
    if os.geteuid() != 0 or mode not in ('check', 'repair') or profile not in ('keep', 'gnome', 'xfce') or instance not in ('dev', 'release'):
        raise RuntimeError('Invalid RDP request')
    if mode == 'repair' and not re.fullmatch('[a-f0-9]{64}', digest):
        raise RuntimeError('Invalid review')
    def interrupted(*_):
        raise RuntimeError('RDP repair interrupted')
    for s in (signal.SIGHUP, signal.SIGTERM, signal.SIGINT):
        signal.signal(s, interrupted)
    report = inspect(profile)[0] if mode == 'check' else repair(profile, digest, instance)
    print(json.dumps(report))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
