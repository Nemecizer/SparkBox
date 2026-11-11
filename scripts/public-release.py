#!/usr/bin/env python3
"""Prepare Developer ID releases. No GitHub commands; submission is explicit."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
NOTARY_ENV = ('APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID', 'APPLE_API_KEY',
              'APPLE_API_ISSUER', 'APPLE_API_KEY_PATH')


def run(args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, **kwargs)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def select_identity(selected, listing):
    available = re.findall(r'^\s*\d+\) [A-Fa-f0-9]+ "([^"]+)"', listing, re.M)
    if not selected or not selected.startswith('Developer ID Application: '):
        raise ValueError('Set SPARKBOX_SIGN_IDENTITY to an exact Developer ID Application identity. Apple Development and Apple Distribution are not accepted.')
    if selected not in available:
        raise ValueError('The selected Developer ID Application identity is not available with a valid private key in Keychain.')
    return selected


def preflight():
    identity = select_identity(os.environ.get('SPARKBOX_SIGN_IDENTITY'),
                               run(['security', 'find-identity', '-v', '-p', 'codesigning']).stdout)
    run(['xcrun', '--find', 'notarytool'])
    run(['xcrun', '--find', 'stapler'])
    print(f'Ready for signing: {identity}. No credentials exported; no submission made.')
    return identity


def private_path_safe_env(original):
    """Preserve compiler flags while removing local home paths from Rust output."""
    env = original.copy()
    if 'CARGO_ENCODED_RUSTFLAGS' in env:
        flags = [flag for flag in env['CARGO_ENCODED_RUSTFLAGS'].split('\x1f') if flag]
    else:
        flags = shlex.split(env.get('RUSTFLAGS', ''))
    # Cargo's encoded form keeps paths containing spaces in one argument.
    flags.append(f'--remap-path-prefix={Path.home()}=/build/home')
    flags.append(f'--remap-path-prefix={ROOT.resolve()}=/build/sparkbox')
    env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(flags)
    env.pop('RUSTFLAGS', None)
    return env


def build(folder):
    identity = preflight()
    if not (ROOT/'LICENSE').is_file():
        raise ValueError('Select and commit the project LICENSE before preparing a public binary.')
    env = private_path_safe_env(os.environ)
    for key in NOTARY_ENV:
        env.pop(key, None)
    env['SPARKBOX_SIGN_IDENTITY'] = identity
    env['APPLE_SIGNING_IDENTITY'] = identity
    # build-candidate enforces clean Git, a new directory, signature and ZIP checks.
    subprocess.run([str(ROOT/'scripts/build-candidate.sh'), folder], cwd=ROOT, env=env, check=True)


def safe_child(root, name):
    p = Path(name)
    if p.is_absolute() or '..' in p.parts or not p.parts:
        raise ValueError('Unsafe provenance path')
    target = root/p
    if not target.resolve().is_relative_to(root.resolve()) or target.is_symlink():
        raise ValueError('Provenance path escapes the candidate')
    return target


def checked_candidate(candidate):
    meta = json.loads((candidate/'PROVENANCE.json').read_text())
    if not re.fullmatch(r'[a-f0-9]{40,64}', meta['source_commit']):
        raise ValueError('Candidate has no valid source commit')
    archive = safe_child(candidate, meta['archive'])
    if archive.parent != candidate or digest(archive) != meta['archive_sha256']:
        raise ValueError('Candidate archive changed since packaging')
    expected = set()
    for entry in meta['files']:
        path = safe_child(candidate, entry['path'])
        if not path.is_relative_to(candidate/'SparkBox.app') or digest(path) != entry['sha256']:
            raise ValueError('Candidate app changed since packaging')
        expected.add(path)
    actual = {p for p in (candidate/'SparkBox.app').rglob('*') if p.is_file()}
    if actual != expected:
        raise ValueError('Candidate file inventory changed since packaging')
    private_prefix = (str(Path.home()) + '/').encode()
    if any(private_prefix in path.read_bytes() for path in actual):
        raise ValueError('Candidate contains local build-machine paths; rebuild with path remapping before submission')
    return meta, archive


def check_signature(app):
    run(['codesign', '--verify', '--deep', '--strict', str(app)])
    for executable in [app, app/'Contents/MacOS/sparkbox-tunnel']:
        details = run(['codesign', '-dv', '--verbose=4', str(executable)]).stderr
        if not re.search(r'^Authority=Developer ID Application: ', details, re.M):
            raise ValueError('Developer ID Application signature is required')
        if '(runtime)' not in details or not re.search(r'^Timestamp=', details, re.M):
            raise ValueError('Hardened runtime and a trusted signing timestamp are required')
        entitlements = run(['codesign', '-d', '--entitlements', ':-', str(executable)]).stdout
        if entitlements.strip() and plistlib.loads(entitlements.encode()).get('com.apple.security.get-task-allow', False):
            raise ValueError('Debug get-task-allow entitlement is forbidden in a public release')


def file_manifest(app):
    return [{'path': str(p.relative_to(app.parent)), 'bytes': p.stat().st_size,
             'mode': p.stat().st_mode & 0o777, 'sha256': digest(p)}
            for p in sorted(app.rglob('*')) if p.is_file()]


def notarize(candidate, output, profile, submit):
    if not submit:
        raise ValueError('Notarization uploads the binary to Apple. Nothing submitted: supply --submit only when explicitly authorized.')
    candidate, output = candidate.resolve(), output.resolve()
    if output.exists() or output.is_symlink():
        raise ValueError('Output must be a new directory; existing candidates are preserved')
    if not profile.strip():
        raise ValueError('A stored Keychain profile name is required')
    meta, archive = checked_candidate(candidate)
    check_signature(candidate/'SparkBox.app')
    output.mkdir(parents=True)
    # Only a profile name is passed; never passwords or private key material.
    try:
        response = run(['xcrun', 'notarytool', 'submit', str(archive), '--keychain-profile', profile,
                        '--wait', '--output-format', 'json'])
    except subprocess.CalledProcessError as error:
        (output/'notary-error.txt').write_text((error.stdout or '') + (error.stderr or ''))
        raise ValueError('Notarization failed; diagnostics retained, no final app produced') from error
    (output/'notary-response.json').write_text(response.stdout)
    reply = json.loads(response.stdout)
    if reply.get('status') != 'Accepted':
        raise ValueError('Apple did not accept this submission; response retained, no final app produced')
    app = output/'SparkBox.app'
    run(['ditto', str(candidate/'SparkBox.app'), str(app)])
    run(['xcrun', 'stapler', 'staple', str(app)])
    run(['xcrun', 'stapler', 'validate', str(app)])
    check_signature(app)
    assessment = run(['spctl', '--assess', '--type', 'execute', '--verbose=2', str(app)])
    (output/'gatekeeper.txt').write_text(assessment.stdout + assessment.stderr)
    final_zip = output/f"SparkBox-{meta['source_commit'][:7]}-notarized.zip"
    run(['ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', str(app), str(final_zip)])
    with tempfile.TemporaryDirectory(prefix='sparkbox-public-verify-') as temp:
        extracted = Path(temp)
        run(['ditto', '-x', '-k', str(final_zip), str(extracted)])
        check_signature(extracted/'SparkBox.app')
        run(['xcrun', 'stapler', 'validate', str(extracted/'SparkBox.app')])
        if file_manifest(app) != file_manifest(extracted/'SparkBox.app'):
            raise ValueError('Final archive files or permissions differ')
    checksum = digest(final_zip)
    (output/'SHA256SUMS').write_text(f'{checksum}  {final_zip.name}\n')
    (output/'PROVENANCE.json').write_text(json.dumps({
        'source_commit': meta['source_commit'], 'source_tree': meta.get('source_tree'),
        'archive': final_zip.name, 'archive_sha256': checksum, 'files': file_manifest(app),
        'notarization': 'Accepted', 'submission_id': reply.get('id'),
        'ticket_stapled_and_validated': True, 'gatekeeper': 'passed',
        'archive_verified': True, 'launched': False, 'published': False,
    }, indent=2)+'\n')
    print(f'Prepared {final_zip}; not launched or published.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('preflight')
    b = sub.add_parser('build'); b.add_argument('folder')
    n = sub.add_parser('notarize')
    n.add_argument('candidate', type=Path); n.add_argument('output', type=Path)
    n.add_argument('--profile', required=True); n.add_argument('--submit', action='store_true')
    args = parser.parse_args()
    try:
        if args.command == 'preflight': preflight()
        elif args.command == 'build': build(args.folder)
        else: notarize(args.candidate, args.output, args.profile, args.submit)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'Stopped: {error}\n')


if __name__ == '__main__':
    main()
