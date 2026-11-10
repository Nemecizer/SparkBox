#!/bin/sh
# Build an identified release candidate without replacing or launching an existing app.
set -eu
cd "$(dirname "$0")/.."

die() { printf '%s\n' "$*" >&2; exit 1; }
if [ "${1:-}" = "--help" ]; then
    printf '%s\n' 'Usage: scripts/build-candidate.sh [folder-name]' \
        'Builds ../SparkBox-builds/<folder-name>/SparkBox.app, ZIP and provenance.' \
        'Default folder: local date plus commit. Requires a clean tree and Apple signing identity.' \
        'Never overwrites a candidate or launches SparkBox. Reuses the Cargo build cache.'
    exit 0
fi
[ "$#" -le 1 ] || die 'Pass at most one candidate folder name.'
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || die 'This packaging procedure targets Apple Silicon macOS.'
[ -z "$(git status --porcelain)" ] || die 'Commit the source and handoff first; the candidate requires a clean Git tree.'
[ -d node_modules ] || die 'Install the locked frontend dependencies with npm ci first.'
python3 -c 'import sys; assert sys.version_info >= (3, 12), "Use Python 3.12 or newer on PATH"'

python3 scripts/audit-distribution.py

candidate_commit=$(git rev-parse HEAD)
candidate_short=$(git rev-parse --short HEAD)
candidate_name=${1:-$(date +%F)-$candidate_short}
case "$candidate_name" in
    ''|.*|*[!A-Za-z0-9._-]*) die 'Use a folder name containing letters, numbers, dots, underscores or hyphens; start with a letter or number.' ;;
esac
case "$candidate_name" in [A-Za-z0-9]*) ;; *) die 'Start the folder name with a letter or number.' ;; esac
candidate_dir="$PWD/../SparkBox-builds/$candidate_name"
[ ! -e "$candidate_dir" ] && [ ! -L "$candidate_dir" ] || die "Already exists: $candidate_dir (choose a new folder name)."
APPLE_SIGNING_IDENTITY=$(scripts/sign-identity.sh)
[ -n "$APPLE_SIGNING_IDENTITY" ] || die 'No Apple signing identity found; see docs/RELEASING.md.'
export APPLE_SIGNING_IDENTITY
CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$PWD/src-tauri/target}
case "$CARGO_TARGET_DIR" in /*) ;; *) CARGO_TARGET_DIR="$PWD/$CARGO_TARGET_DIR" ;; esac
export CARGO_BUILD_JOBS CARGO_TARGET_DIR

mkdir -p "$PWD/../SparkBox-builds"
mkdir "$candidate_dir"
mkdir "$candidate_dir/evidence"
candidate_stage=$(mktemp -d "${TMPDIR:-/tmp}/sparkbox-bundle.XXXXXX")
trap 'rm -rf "$candidate_stage"' 0
trap 'exit 130' INT
trap 'exit 143' HUP TERM
printf 'Building %s; log: %s/evidence/build.log\n' "$candidate_short" "$candidate_dir"
if ! npm run build:app -- --no-bundle > "$candidate_dir/evidence/build.log" 2>&1; then
    die "Build failed; inspect $candidate_dir/evidence/build.log. Partial output is retained."
fi
[ "$(git rev-parse HEAD)" = "$candidate_commit" ] && [ -z "$(git status --porcelain)" ] || die 'Source changed during the build; no candidate was packaged.'
mkdir "$candidate_stage/release"
cp "$CARGO_TARGET_DIR/release/sparkbox" "$candidate_stage/release/sparkbox"
if ! CARGO_TARGET_DIR="$candidate_stage" npx tauri bundle --bundles app --ci > "$candidate_dir/evidence/package.log" 2>&1; then
    die "Packaging failed; inspect $candidate_dir/evidence/package.log."
fi
codesign --verify --deep --strict "$candidate_stage/release/bundle/macos/SparkBox.app"
ditto "$candidate_stage/release/bundle/macos/SparkBox.app" "$candidate_dir/SparkBox.app"
codesign --verify --deep --strict "$candidate_dir/SparkBox.app"
ditto -c -k --sequesterRsrc --keepParent "$candidate_dir/SparkBox.app" "$candidate_dir/SparkBox-$candidate_short.zip"
mkdir "$candidate_stage/extracted"
ditto -x -k "$candidate_dir/SparkBox-$candidate_short.zip" "$candidate_stage/extracted"
codesign --verify --deep --strict "$candidate_stage/extracted/SparkBox.app"
[ "$(git rev-parse HEAD)" = "$candidate_commit" ] && [ -z "$(git status --porcelain)" ] || die 'Source changed during packaging; do not distribute this candidate.'
python3 scripts/candidate-provenance.py "$candidate_dir" "$candidate_stage/extracted"
printf 'Built and verified: %s/SparkBox.app\nSparkBox was not launched.\n' "$candidate_dir"
