#!/bin/sh
# Everything that has to pass before a change is done: types, UI tests (the
# colour policy and type floor over every stylesheet included), Rust tests (fake
# SSH nodes included), the Python tests of every node script and the VPN
# helper's Go tests.
# Runs every step even after a failure and prints a summary at the end.
# Set CARGO_TARGET_DIR to keep cargo's build separate from the dev app's.
# Fake SSH/model tests start several real processes per test. CPU-count parallelism can
# exhaust their short setup windows on large Macs. Keep the standard check bounded;
# RUST_TEST_THREADS=18 npm run check (for example) explicitly opts into a stress run.
: "${RUST_TEST_THREADS:=1}"
export RUST_TEST_THREADS
cd "$(dirname "$0")/.." || exit 1
failed=""

step() {
  name="$1"
  shift
  printf '\n== %s\n' "$name"
  if "$@"; then
    printf -- '-- %s: ok\n' "$name"
  else
    printf -- '-- %s: FAILED\n' "$name"
    failed="$failed $name"
  fi
}

step "distribution audit tests" python3 -m unittest discover -s scripts -p distribution_audit_test.py
step "public release tools" python3 -m unittest discover -s scripts -p public_release_test.py
step "offline cancellation replay" python3 -m unittest discover -s scripts -p cancel_replay_test.py
step "typescript" npx tsc --noEmit
step "vitest" npx vitest run
step "vpn helper build" npm run build:tunnel
step "cargo" sh -c 'cd src-tauri && cargo test --quiet'
# The release build's write path in a real release compile: what SPARKBOX_ADMIN_NOAUTH can't do
# there, and what needs no fence, can't be seen from a debug build (live/exit_release_tests.rs).
step "cargo release path" sh -c 'cd src-tauri && cargo test --release --quiet --lib -- release_tests release_exit_tests'
# Every node script's tests, wherever they live (live/, agent/…), each run from its own folder.
step "python" sh -c 'for t in $(find src-tauri/src -name "*_test.py"); do (cd "$(dirname "$t")" && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -q "$(basename "${t%.py}")") || exit 1; done'
step "go" sh -c 'cd tunnel && go test ./...'

if [ -n "$failed" ]; then
  printf '\nFailed:%s\n' "$failed"
  exit 1
fi
printf '\nAll checks passed.\n'
