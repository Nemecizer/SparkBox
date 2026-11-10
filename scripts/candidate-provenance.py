"""Record source and artifact hashes after build-candidate.sh verifies signatures."""
import datetime
import hashlib
import json
import pathlib
import platform
import plistlib
import subprocess
import sys


def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


out, extracted = (pathlib.Path(p) for p in sys.argv[1:])
app = out / 'SparkBox.app'
files = []
for path in sorted(app.rglob('*')):
    if not path.is_file():
        continue
    rel = path.relative_to(out)
    other = extracted / rel
    digest = sha(path)
    if digest != sha(other) or path.stat().st_mode & 0o777 != other.stat().st_mode & 0o777:
        raise SystemExit(f'ZIP verification failed: {rel}')
    files.append({'path': str(rel), 'bytes': path.stat().st_size, 'sha256': digest})
with (app / 'Contents/Info.plist').open('rb') as stream:
    info = plistlib.load(stream)
archive = out / f'SparkBox-{git("rev-parse", "--short", "HEAD")}.zip'
record = {
    'created_at': datetime.datetime.now().astimezone().isoformat(),
    'source_commit': git('rev-parse', 'HEAD'),
    'source_tree': git('rev-parse', 'HEAD^{tree}'),
    'build_tag': git('describe', '--tags', '--always', '--dirty'),
    'branch': git('branch', '--show-current'),
    'version': info['CFBundleShortVersionString'],
    'identifier': info['CFBundleIdentifier'],
    'architecture': platform.machine(),
    'archive': archive.name,
    'archive_sha256': sha(archive),
    'archive_bytes': archive.stat().st_size,
    'files': files,
    'archive_verified_files': len(files),
    'signature': 'codesign --verify --deep --strict passed for staged, delivered and ZIP-extracted app',
    'notarization': 'Not checked by this script; inspect evidence/package.log',
    'checks': ['Help release lint', 'TypeScript and production web build', 'release compile', 'signatures', 'ZIP file hashes and permissions'],
    'test_suites': 'Run separately; packaging does not establish a full-suite pass',
    'hardware_acceptance': 'Not run by this script; see docs/RELEASE-NOTES.md for evidence and limits',
    'launched_against_cluster': False,
}
(out / 'PROVENANCE.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps({'source_commit': record['source_commit'], 'verified_files': len(files)}))
