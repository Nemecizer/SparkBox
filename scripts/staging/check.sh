#!/bin/sh
# Runs the node scripts' Python tests under real Python 3.12 (PLAN P1.S) — the 3.12 run round 1
# owed, which the Mac couldn't do (it has 3.15, not 3.12, and no uv). It runs them INSIDE the
# staging container (python3 is 3.12.3 there), as the cluster user sparkuser, the way the collector
# and sparkjob run on a node.
#
#   scripts/staging/check.sh
#
# Ensures the container (scripts/staging/setup.sh), copies src-tauri/src into it, and runs
# sparkjob/launch/collector/roce and the agent suite (serving_probe, agent) with `python3 -m unittest`.
# Prints each suite's result and an overall summary. A suite passes only when unittest ran tests
# and said OK; no result at all (the container stopped, python3 missing, the run killed, no tests
# found) is a failure. Exits non-zero if a suite fails.
#
# agent_test.RunAsAService runs the agent under real systemd here: it passes since the agent keeps
# NOTIFY_SOCKET from the systemd tools it starts (keep_notify_socket; README.md finding 2).
#
# One user of the container at a time: this holds the same lock as the Rust staging tests
# (src-tauri/src/live/staging_tests.rs), so it waits while they run, and they wait for it.
#
# uv (optional) would let the suite run on the Mac under 3.12 too; it is not installed and this
# script installs nothing. Docker Desktop must be started once by hand if setup.sh reports it can't
# (exit 2 here too).
set -eu
unset DOCKER_HOST
DOCKER="docker --context desktop-linux"
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
CONTAINER=sparkbox-staging-spark1

# The container's use lock (flock, released by the kernel when this script ends however it ends).
tmp=${TMPDIR:-/tmp}
lock="${tmp%/}/$CONTAINER.lock"
exec 9>>"$lock"
if ! lockf -s -t 0 9; then
  echo "The staging tests or another check.sh are using $CONTAINER; waiting for them (up to 15 minutes)…"
  lockf -s -t 900 9 || { echo "$CONTAINER was still in use after 15 minutes. Nothing checked." >&2; exit 1; }
fi

"$HERE/setup.sh" >/dev/null || {
  status=$?
  [ "$status" = 2 ] && { echo "Docker Desktop isn't up; start it by hand and re-run. Nothing checked."; exit 2; }
  echo "Couldn't bring up the staging container." >&2
  exit "$status"
}

echo "Copying src-tauri/src into ${CONTAINER}…"
$DOCKER exec "$CONTAINER" sh -c 'rm -rf /opt/sb && mkdir -p /opt/sb'
$DOCKER cp "$REPO/src-tauri/src" "$CONTAINER:/opt/sb/src" >/dev/null
$DOCKER exec "$CONTAINER" sh -c 'chmod -R a+rX /opt/sb/src'
pyver=$($DOCKER exec "$CONTAINER" python3 --version)
echo "Running the node tests under $pyver, as sparkuser.\n"

# suite <dir> <module>
failed=""
run_suite() {
  dir="$1"
  mod="$2"
  printf '== %s/%s\n' "$dir" "$mod"
  rc=0
  out=$($DOCKER exec -u sparkuser -w "/opt/sb/src/$dir" "$CONTAINER" python3 -m unittest -q "$mod" 2>&1) || rc=$?
  # The `Ran N tests` line and the OK / FAILED verdict.
  printf '%s\n' "$out" | grep -E '^Ran |^OK|^FAILED' | sed 's/^/   /'
  ran=$(printf '%s\n' "$out" | sed -n -E 's/^Ran ([0-9]+) tests? in .*/\1/p' | head -1)
  verdict=$(printf '%s\n' "$out" | grep -E '^OK|^FAILED' | head -1)
  # No result at all (docker exec failed, python3 missing, the run was killed, no tests found).
  if [ -z "$ran" ] || [ "$ran" = 0 ] || [ -z "$verdict" ]; then
    echo "   no unittest result (exit $rc): $(printf '%s\n' "$out" | grep -v '^[[:space:]]*$' | head -2 | tr '\n' ' ')"
    failed="$failed $dir/$mod"
    return 0
  fi
  if printf '%s' "$verdict" | grep -q '^OK' && [ "$rc" = 0 ]; then
    return 0
  fi
  # Which tests failed. A FAILED verdict that names no test (an unexpected success) fails too.
  bad=$(printf '%s\n' "$out" | grep -E '^(FAIL|ERROR): ' | sed -E 's/^(FAIL|ERROR): ([^ ]+).*/\2/')
  for name in $bad; do
    echo "   failing: $name"
  done
  [ -n "$bad" ] || echo "   failed without naming a test: $verdict (exit $rc)"
  failed="$failed $dir/$mod"
}

run_suite live sparkjob_test
run_suite live launch_test
run_suite live collector_test
run_suite live roce_test
run_suite agent serving_probe_test
run_suite agent agent_test

echo
if [ -n "$failed" ]; then
  echo "FAILED:$failed"
  exit 1
fi
echo "Python 3.12 node tests passed."
