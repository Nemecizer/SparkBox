#!/usr/bin/env python3
"""Fail closed on common accidental source-export inclusions; never print values.

This is a packaging guard, not a replacement for a secret scanner and human review.
An optional private JSON list of forbidden strings must stay outside the source tree.
"""
import argparse
import json
from pathlib import Path, PurePosixPath
import re
import subprocess

PRIVATE_KEY = re.compile(rb'-----BEGIN (?:OPENSSH |RSA |EC |DSA |ENCRYPTED )?PRIVATE KEY-----[ \t]*\r?\n')
PRIVATE_DIRS = {'.git', '.ssh', '.aws', '.codex', 'node_modules', 'target', 'acceptance', 'continuation', 'evidence'}
STATE_FILES = {'inventory.json', 'prefs.json', 'providers.json', 'vpn.json', 'security.json', 'known_hosts', 'secrets.json'}


def audit(root, paths, forbidden=()):
    issues = []
    for name in paths:
        rel = PurePosixPath(name)
        path = root / name
        if rel.is_absolute() or '..' in rel.parts or path.is_symlink() or not path.is_file():
            issues.append((name, 'unsafe or missing file'))
            continue
        if any(p in PRIVATE_DIRS for p in rel.parts) or rel.name.startswith('.env') or rel.suffix.lower() in {'.p12', '.pfx', '.p8', '.key', '.ppk', '.mobileprovision'}:
            issues.append((name, 'private state, key or generated dependency path'))
        if rel.name in STATE_FILES and str(rel) != 'third-party/inventory.json' and 'fixtures' not in rel.parts and 'tests' not in rel.parts:
            issues.append((name, 'saved connection or application state'))
        data = path.read_bytes()
        if PRIVATE_KEY.search(data):
            issues.append((name, 'embedded private key'))
        if any(value and value.encode() in data for value in forbidden):
            issues.append((name, 'private identifier from external deny list'))
    return issues


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deny-file', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    forbidden = []
    if args.deny_file:
        deny = args.deny_file.resolve()
        if deny.is_relative_to(root):
            parser.error('Keep the private deny file outside the publishing source.')
        forbidden = json.loads(deny.read_text())
        if not isinstance(forbidden, list) or not all(isinstance(v, str) and v for v in forbidden):
            parser.error('The private deny file must be a JSON list of nonempty strings.')
    names = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).decode().split('\0')
    names = [n for n in names if n]
    if not names:
        parser.error('No tracked source files to audit.')
    issues = audit(root, names, forbidden)
    for name, reason in issues:
        print(f'{name}: {reason}')
    print(f'Publication guard: {len(names)} files, {len(issues)} findings. Review synthetic fixtures and run a secret scan separately.')
    return bool(issues)


if __name__ == '__main__':
    raise SystemExit(main())
